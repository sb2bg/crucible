//! Git operations for managing engine repositories.
//!
//! Handles cloning, fetching, enumerating commits across branches,
//! identifying tags/releases, and building engine binaries from source.

use anyhow::{Context, Result};
use git2::{BranchType, Repository, Sort};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

use crate::types::*;

const DISPLAY_HASH_LEN: usize = 12;

pub fn short_hash(hash: &str) -> &str {
    &hash[..DISPLAY_HASH_LEN.min(hash.len())]
}

/// Manages git operations for a single engine repository
pub struct GitManager {
    pub repo_url: String,
    pub local_path: PathBuf,
    pub build_cmd: String,
    pub binary_path: String,
}

impl GitManager {
    pub fn new(repo_url: &str, local_path: &Path, build_cmd: &str, binary_path: &str) -> Self {
        Self {
            repo_url: repo_url.to_string(),
            local_path: local_path.to_path_buf(),
            build_cmd: build_cmd.to_string(),
            binary_path: binary_path.to_string(),
        }
    }

    /// Clone or open the repository
    pub fn ensure_repo(&self) -> Result<Repository> {
        if self.local_path.exists() {
            info!("Opening existing repo at {:?}", self.local_path);
            let repo =
                Repository::open(&self.local_path).context("Failed to open existing repository")?;
            // Fetch latest changes
            self.fetch(&repo)?;
            Ok(repo)
        } else {
            info!("Cloning {} to {:?}", self.repo_url, self.local_path);
            let repo = Repository::clone(&self.repo_url, &self.local_path)
                .context("Failed to clone repository")?;
            Ok(repo)
        }
    }

    /// Fetch all remotes
    fn fetch(&self, repo: &Repository) -> Result<()> {
        let mut remote = repo
            .find_remote("origin")
            .context("No 'origin' remote found")?;
        remote.fetch(&[] as &[&str], None, None)?;
        Ok(())
    }

    /// Enumerate all commits on a branch, ordered oldest-first
    pub fn list_commits(
        &self,
        repo: &Repository,
        branch_name: &str,
        engine_id: &str,
        since: Option<&str>,
    ) -> Result<Vec<EngineRevision>> {
        let branch = repo
            .find_branch(&format!("origin/{}", branch_name), BranchType::Remote)
            .context(format!("Branch 'origin/{}' not found", branch_name))?;

        let branch_oid = branch.get().target().context("Branch has no target")?;
        let tags = self.collect_tags(repo)?;

        let mut revwalk = repo.revwalk()?;
        revwalk.push(branch_oid)?;
        revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?; // oldest first

        let mut revisions = Vec::new();
        let mut started = since.is_none();
        let mut matched_start = since.is_none();

        for oid_result in revwalk {
            let oid = oid_result?;
            let commit = repo.find_commit(oid)?;
            let hash = oid.to_string();

            // If we have a start point, skip until we find it
            if !started {
                if let Some(start) = since {
                    if hash.starts_with(start) || tags.get(&hash).map_or(false, |t| t == start) {
                        started = true;
                        matched_start = true;
                    } else {
                        continue;
                    }
                }
            }

            let tag = tags.get(&hash).cloned();
            let is_release = tag.as_ref().map_or(false, |t| {
                t.starts_with('v') || t.starts_with('V') || t.contains("release")
            });

            let message = commit
                .message()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();

            let timestamp = commit.time().seconds();
            let commit_date = chrono::DateTime::from_timestamp(timestamp, 0)
                .unwrap_or_default()
                .with_timezone(&chrono::Utc);

            let rev = EngineRevision {
                id: format!("{}-{}", engine_id, hash),
                engine_id: engine_id.to_string(),
                commit_hash: hash,
                commit_message: message,
                commit_date,
                branch: branch_name.to_string(),
                tag,
                is_release,
                binary_path: None,
                build_status: BuildStatus::Pending,
            };
            revisions.push(rev);
        }

        if let Some(start) = since {
            if !matched_start {
                anyhow::bail!(
                    "start_from '{}' was not found on branch '{}'",
                    start,
                    branch_name
                );
            }
        }

        Ok(revisions)
    }

    /// Collect all tags mapping commit hash -> tag name
    fn collect_tags(&self, repo: &Repository) -> Result<std::collections::HashMap<String, String>> {
        let mut tags = std::collections::HashMap::new();
        repo.tag_foreach(|oid, name| {
            let name = String::from_utf8_lossy(name)
                .trim_start_matches("refs/tags/")
                .to_string();
            // Resolve annotated tags to their target commit
            if let Ok(obj) = repo.find_object(oid, None) {
                if let Ok(commit) = obj.peel_to_commit() {
                    tags.insert(commit.id().to_string(), name.clone());
                }
            }
            tags.insert(oid.to_string(), name);
            true
        })?;
        Ok(tags)
    }

    /// Checkout a specific commit and build the engine
    pub fn build_revision(&self, repo: &Repository, commit_hash: &str) -> Result<PathBuf> {
        // Checkout the commit
        let oid = git2::Oid::from_str(commit_hash)?;
        repo.set_head_detached(oid)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;

        info!("Building commit {}...", short_hash(commit_hash));

        // Build commands are configured as arbitrary shell snippets.
        let output = Command::new("sh")
            .arg("-lc")
            .arg(&self.build_cmd)
            .current_dir(&self.local_path)
            .output()
            .context("Failed to execute build command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Build failed for {}: {}", short_hash(commit_hash), stderr);
        }

        // Copy the built binary to a versioned location
        let src_binary = self.local_path.join(&self.binary_path);
        if !src_binary.exists() {
            anyhow::bail!("Binary not found at {:?} after build", src_binary);
        }

        let dest_dir = self.local_path.join(".crucible-builds");
        std::fs::create_dir_all(&dest_dir)?;
        let dest = dest_dir.join(format!("engine-{}", short_hash(commit_hash)));
        std::fs::copy(&src_binary, &dest)?;

        // Make it executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dest)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms)?;
        }

        info!("Built {} -> {:?}", short_hash(commit_hash), dest);
        Ok(dest)
    }

    /// Get the list of commits between two hashes (for bisect)
    pub fn commits_between(
        &self,
        repo: &Repository,
        old_hash: &str,
        new_hash: &str,
    ) -> Result<Vec<String>> {
        let old_oid = git2::Oid::from_str(old_hash)?;
        let new_oid = git2::Oid::from_str(new_hash)?;

        let mut revwalk = repo.revwalk()?;
        revwalk.push(new_oid)?;
        revwalk.hide(old_oid)?;
        revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;

        let mut hashes = vec![old_oid.to_string()];
        hashes.extend(
            revwalk
                .filter_map(|oid| oid.ok().map(|o| o.to_string()))
                .collect::<Vec<_>>(),
        );

        Ok(hashes)
    }
}

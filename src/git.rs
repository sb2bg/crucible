//! Git operations for managing engine repositories.
//!
//! Handles cloning, fetching, enumerating commits across branches,
//! identifying tags/releases, and building engine binaries from source.

use anyhow::{Context, Result};
use git2::{BranchType, Delta, DiffFormat, Oid, Repository, Sort};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

use crate::types::*;

const DISPLAY_HASH_LEN: usize = 12;
const MAX_PATCH_BYTES: usize = 24 * 1024;

pub fn short_hash(hash: &str) -> &str {
    &hash[..DISPLAY_HASH_LEN.min(hash.len())]
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitActor {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffFileSummary {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffSummary {
    pub base_hash: Option<String>,
    pub head_hash: String,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub files: Vec<DiffFileSummary>,
    pub patch: String,
    pub patch_truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitDetails {
    pub full_message: String,
    pub body: Option<String>,
    pub author: CommitActor,
    pub committer: CommitActor,
    pub author_time: chrono::DateTime<chrono::Utc>,
    pub parent_hashes: Vec<String>,
}

fn actor_from_signature(signature: &git2::Signature<'_>) -> CommitActor {
    CommitActor {
        name: signature.name().map(str::to_string),
        email: signature.email().map(str::to_string),
    }
}

fn timestamp_to_utc(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Utc)
}

fn delta_status(status: Delta) -> &'static str {
    match status {
        Delta::Unmodified => "unmodified",
        Delta::Added => "added",
        Delta::Deleted => "deleted",
        Delta::Modified => "modified",
        Delta::Renamed => "renamed",
        Delta::Copied => "copied",
        Delta::Ignored => "ignored",
        Delta::Untracked => "untracked",
        Delta::Typechange => "typechange",
        Delta::Unreadable => "unreadable",
        Delta::Conflicted => "conflicted",
    }
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
            debug!("Opening existing repo at {:?}", self.local_path);
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
        remote.fetch(&["+refs/heads/*:refs/remotes/origin/*"], None, None)?;
        Ok(())
    }

    pub fn resolve_branch_patterns(
        &self,
        repo: &Repository,
        configured: &[String],
    ) -> Result<Vec<String>> {
        let remote_branches = self.remote_branch_names(repo)?;
        let mut resolved = Vec::new();

        for pattern in configured {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }

            if has_branch_wildcard(pattern) {
                let mut matches = remote_branches
                    .iter()
                    .filter(|branch| branch_pattern_matches(pattern, branch))
                    .cloned()
                    .collect::<Vec<_>>();
                matches.sort();
                matches.dedup();

                if matches.is_empty() {
                    warn!("Branch pattern '{}' matched no remote branches", pattern);
                }

                resolved.extend(matches);
            } else {
                if !remote_branches.iter().any(|branch| branch == pattern) {
                    anyhow::bail!("Branch 'origin/{}' not found", pattern);
                }
                resolved.push(pattern.to_string());
            }
        }

        resolved.sort();
        resolved.dedup();
        Ok(resolved)
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
                    if hash.starts_with(start) || tags.get(&hash).is_some_and(|t| t == start) {
                        started = true;
                        matched_start = true;
                    } else {
                        continue;
                    }
                }
            }

            let tag = tags.get(&hash).cloned();
            let is_release = tag
                .as_ref()
                .is_some_and(|t| t.starts_with('v') || t.starts_with('V') || t.contains("release"));

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
                binary_fingerprint: None,
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

    fn remote_branch_names(&self, repo: &Repository) -> Result<Vec<String>> {
        let mut branches = Vec::new();
        for branch in repo.branches(Some(BranchType::Remote))? {
            let (branch, _) = branch?;
            let Some(name) = branch.name()? else {
                continue;
            };
            if name == "origin/HEAD" {
                continue;
            }
            let Some(stripped) = name.strip_prefix("origin/") else {
                continue;
            };
            branches.push(stripped.to_string());
        }
        branches.sort();
        branches.dedup();
        Ok(branches)
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

    pub fn fingerprint_binary(path: &Path) -> Result<String> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read built binary '{}'", path.display()))?;
        let digest = Sha256::digest(bytes);
        Ok(format!("{:x}", digest))
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

    pub fn commit_details(&self, repo: &Repository, commit_hash: &str) -> Result<CommitDetails> {
        let oid = Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let full_message = commit.message().unwrap_or("").trim_end().to_string();
        let body = commit
            .body()
            .map(str::trim)
            .filter(|body| !body.is_empty())
            .map(str::to_string);
        let author = actor_from_signature(&commit.author());
        let committer = actor_from_signature(&commit.committer());

        Ok(CommitDetails {
            full_message,
            body,
            author,
            committer,
            author_time: timestamp_to_utc(commit.time().seconds()),
            parent_hashes: commit.parent_ids().map(|oid| oid.to_string()).collect(),
        })
    }

    pub fn diff_for_revision(&self, repo: &Repository, commit_hash: &str) -> Result<DiffSummary> {
        let oid = Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let base_hash = commit.parent_id(0).ok().map(|oid| oid.to_string());
        self.diff_between_oids(repo, base_hash.as_deref(), Some(commit_hash))
    }

    pub fn diff_between(
        &self,
        repo: &Repository,
        base_hash: &str,
        head_hash: &str,
    ) -> Result<DiffSummary> {
        self.diff_between_oids(repo, Some(base_hash), Some(head_hash))
    }

    fn diff_between_oids(
        &self,
        repo: &Repository,
        base_hash: Option<&str>,
        head_hash: Option<&str>,
    ) -> Result<DiffSummary> {
        let base_tree = base_hash
            .map(|hash| self.tree_for_commit(repo, hash))
            .transpose()?;
        let head_hash = head_hash.context("missing head revision")?;
        let head_tree = self.tree_for_commit(repo, head_hash)?;

        let diff = repo.diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), None)?;
        let stats = diff.stats()?;

        let mut files = Vec::new();
        for index in 0..diff.deltas().len() {
            let delta = diff
                .get_delta(index)
                .context("missing delta while building diff summary")?;
            let (additions, deletions) = git2::Patch::from_diff(&diff, index)?
                .and_then(|patch| patch.line_stats().ok())
                .map(|(_, additions, deletions)| (additions, deletions))
                .unwrap_or((0, 0));

            files.push(DiffFileSummary {
                old_path: delta
                    .old_file()
                    .path()
                    .map(|path| path.display().to_string()),
                new_path: delta
                    .new_file()
                    .path()
                    .map(|path| path.display().to_string()),
                status: delta_status(delta.status()).to_string(),
                additions,
                deletions,
            });
        }

        let mut patch = String::new();
        let mut patch_bytes = 0usize;
        let mut patch_truncated = false;
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            if patch_truncated {
                return true;
            }
            let content = String::from_utf8_lossy(line.content());
            let content_bytes = content.len();
            if patch_bytes + content_bytes > MAX_PATCH_BYTES {
                patch_truncated = true;
                return true;
            }
            patch.push_str(&content);
            patch_bytes += content_bytes;
            true
        })?;
        if patch_truncated {
            patch.push_str("\n... diff truncated ...\n");
        }

        Ok(DiffSummary {
            base_hash: base_hash.map(str::to_string),
            head_hash: head_hash.to_string(),
            files_changed: stats.files_changed(),
            insertions: stats.insertions(),
            deletions: stats.deletions(),
            files,
            patch,
            patch_truncated,
        })
    }

    fn tree_for_commit<'repo>(
        &self,
        repo: &'repo Repository,
        commit_hash: &str,
    ) -> Result<git2::Tree<'repo>> {
        let oid = Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        Ok(commit.tree()?)
    }
}

fn has_branch_wildcard(pattern: &str) -> bool {
    pattern.contains('*')
}

pub fn branch_pattern_matches(pattern: &str, candidate: &str) -> bool {
    if !has_branch_wildcard(pattern) {
        return pattern == candidate;
    }

    let parts = pattern.split('*').collect::<Vec<_>>();
    let mut remainder = candidate;
    let mut anchored_start = true;

    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            anchored_start = false;
            continue;
        }

        if index == 0 && anchored_start {
            let Some(stripped) = remainder.strip_prefix(part) else {
                return false;
            };
            remainder = stripped;
            continue;
        }

        if index == parts.len() - 1 {
            return remainder.ends_with(part);
        }

        let Some(position) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[position + part.len()..];
    }

    pattern.ends_with('*') || remainder.is_empty()
}

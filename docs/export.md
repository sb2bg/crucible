---
title: Exporting results
nav_order: 14
---

# Exporting results

`crucible export` writes the full state of the database to a single JSON bundle that external tools can consume without touching SQLite.

```bash
crucible export
crucible export --output /tmp/crucible.json
```

Without `--output`, the file is named `crucible-export-<timestamp>.json` and lands in the current working directory.

The bundle includes every tracked engine, every revision known to the database, every test job with its stopping-rule outcome, every completed game with its PGN, every bisect session, and every training run recorded on disk. The structure is stable enough to feed into downstream analysis scripts or LLM prompts.

The admin tab of the web dashboard has a Download export button that returns the same JSON bundle. If you have set `server.admin_token`, the button sends the token as a bearer on the underlying `/api/admin/export` request. A token is required when the dashboard binds to a non-loopback address.

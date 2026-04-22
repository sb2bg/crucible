---
title: Dashboards
nav_order: 11
---

# Dashboards

Crucible ships with a web dashboard and an optional terminal UI. Both read from the same SQLite database, so you can run either, both, or neither without affecting the daemon.

## The web dashboard

By default the dashboard listens on <http://localhost:8877>. It is a single page served by an embedded Axum server, with tabs across the top:

- **Overview**: engines, latest revisions, the most recent jobs, and daemon status at a glance.
- **Timeline**: estimated Elo over time, with confidence intervals. Tagged releases are highlighted.
- **Jobs**: every queued, running, and completed job on the canonical branches.
- **Experiments**: active and historical jobs for branches listed under `experimental_branches`.
- **Bisect**: running and completed regression hunts.
- **Training**: self-play export runs and depth-bucket counts.
- **Gate**: completed and running release gates, with download links for each summary.
- **Admin**: add and remove engines, queue manual tests, start regression hunts, cancel jobs, and download the JSON export bundle.

If `server.admin_token` is set, the Admin tab prompts for the token and stores it in the browser's local storage.

## The terminal UI

Run the terminal UI alongside the daemon with `crucible run --tui`, or attach it to a running daemon with `crucible monitor`. The TUI has four tabs:

| Key | Tab       | Shows                                                         |
| --- | --------- | ------------------------------------------------------------- |
| `1` | Dashboard | A compact summary of engines, queued jobs, and daemon status. |
| `2` | Jobs      | Every job the daemon knows about, oldest first.               |
| `3` | Timeline  | A text rendering of the Elo timeline per engine.              |
| `4` | Bisect    | Active and recent regression hunts.                           |

`Tab` cycles through the four views. `q` quits the UI without stopping the daemon.

## HTTP API

The dashboard is backed by a small HTTP API. Public routes live under `/api/` and are safe to expose. Admin routes live under `/api/admin/` and are protected by `server.admin_token` when it is set.

Public routes:

- `GET /api/health`
- `GET /api/status`
- `GET /api/engines`
- `GET /api/timeline/:engine_id`
- `GET /api/jobs`
- `GET /api/bisect`
- `GET /api/training/runs`
- `GET /api/revisions/:engine_id/:revision_ref`
- `GET /api/compare/:engine_id`

Admin routes:

- `POST /api/admin/engines`, `DELETE /api/admin/engines/:engine_id`
- `POST /api/admin/tests`
- `POST /api/admin/bisect`, `POST /api/admin/bisect/:session_id/cancel`
- `POST /api/admin/jobs/:job_id/cancel`
- `GET /api/admin/gates`, `POST /api/admin/gates`, `GET /api/admin/gates/:file_name`, `POST /api/admin/gates/:gate_id/cancel`
- `GET /api/admin/export`

Admin requests need an `Authorization: Bearer <token>` header when `server.admin_token` is set.

## Health checks

`GET /api/health` returns a small unauthenticated JSON response:

```json
{
  "status": "ok",
  "timestamp": "2026-04-22T12:00:00Z"
}
```

The health endpoint deliberately avoids SQLite and other shared daemon state. Use it to separate web-server liveness from storage-backed API health:

- If `/api/health` responds but `/api/status` hangs, the Axum server is reachable and the issue is likely in SQLite or another shared resource.
- If `/api/health` also hangs, the web runtime is likely starved or the container/network path is blocked.
- If `/api/health` is refused, the process is down or not listening on the configured address.

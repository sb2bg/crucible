//! Web server for the Crucible dashboard.
//!
//! Provides a browser-based UI with:
//! - Elo timeline chart (the main view)
//! - Live game viewer via WebSocket
//! - Job queue management
//! - Bisect visualization
//!
//! The web UI is served as a single embedded HTML page
//! with all JS/CSS inline for zero-dependency deployment.

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::info;

use crate::storage::Storage;

pub struct WebState {
    pub storage: Storage,
}

pub fn create_router(storage: Storage) -> Router {
    let state = Arc::new(WebState { storage });

    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/engines", get(engines_handler))
        .route("/api/timeline/{engine_id}", get(timeline_handler))
        .route("/api/jobs", get(jobs_handler))
        .with_state(state)
}

async fn index_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn status_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_system_status() {
        Ok(status) => Json(serde_json::to_value(status).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn engines_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_engines() {
        Ok(engines) => Json(serde_json::to_value(engines).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn timeline_handler(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(engine_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.storage.get_elo_timeline(&engine_id, None) {
        Ok(timeline) => Json(serde_json::to_value(timeline).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn jobs_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    // Return recent jobs
    Json(serde_json::json!({"jobs": []}))
}

/// The entire dashboard as a single embedded HTML page.
/// This keeps deployment dead simple — no static file serving needed.
const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Crucible — Chess Engine CI</title>
<style>
  :root {
    --bg: #0d1117; --surface: #161b22; --border: #30363d;
    --text: #e6edf3; --text-dim: #8b949e; --accent: #58a6ff;
    --green: #3fb950; --red: #f85149; --yellow: #d29922;
    --font: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { background: var(--bg); color: var(--text); font-family: var(--font); font-size: 14px; }
  .header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 16px 24px; border-bottom: 1px solid var(--border);
  }
  .header h1 { font-size: 18px; color: var(--accent); }
  .header .status { color: var(--green); font-size: 12px; }
  .tabs {
    display: flex; gap: 0; border-bottom: 1px solid var(--border);
    padding: 0 24px; background: var(--surface);
  }
  .tab {
    padding: 10px 20px; cursor: pointer; color: var(--text-dim);
    border-bottom: 2px solid transparent; transition: all 0.2s;
  }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--accent); border-bottom-color: var(--accent); }
  .content { padding: 24px; }
  .grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 16px; margin-bottom: 24px; }
  .stat-card {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: 8px; padding: 16px;
  }
  .stat-card .label { font-size: 12px; color: var(--text-dim); text-transform: uppercase; }
  .stat-card .value { font-size: 28px; font-weight: bold; margin-top: 4px; }
  .chart-container {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: 8px; padding: 24px; min-height: 400px;
  }
  .chart-container h2 { font-size: 16px; margin-bottom: 16px; color: var(--text-dim); }
  #elo-chart { width: 100%; height: 350px; }
  .jobs-table {
    width: 100%; border-collapse: collapse;
    background: var(--surface); border: 1px solid var(--border); border-radius: 8px;
  }
  .jobs-table th {
    text-align: left; padding: 12px 16px; font-size: 12px;
    color: var(--text-dim); text-transform: uppercase;
    border-bottom: 1px solid var(--border);
  }
  .jobs-table td { padding: 10px 16px; border-bottom: 1px solid var(--border); }
  .badge {
    display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px;
  }
  .badge.running { background: #1f3a1f; color: var(--green); }
  .badge.queued { background: #1f2a3f; color: var(--accent); }
  .badge.completed { background: #1f1f1f; color: var(--text-dim); }
  .badge.h1 { background: #1f3a1f; color: var(--green); }
  .badge.h0 { background: #3a1f1f; color: var(--red); }
  canvas { image-rendering: auto; }
</style>
</head>
<body>

<div class="header">
  <h1>⚗ Crucible</h1>
  <div class="status" id="status-indicator">● Connected</div>
</div>

<div class="tabs">
  <div class="tab active" data-tab="dashboard">Dashboard</div>
  <div class="tab" data-tab="timeline">Elo Timeline</div>
  <div class="tab" data-tab="jobs">Jobs</div>
  <div class="tab" data-tab="bisect">Bisect</div>
</div>

<div class="content" id="content">
  <div class="grid">
    <div class="stat-card">
      <div class="label">Engines Tracked</div>
      <div class="value" id="stat-engines" style="color: var(--accent)">-</div>
    </div>
    <div class="stat-card">
      <div class="label">Active Tests</div>
      <div class="value" id="stat-active" style="color: var(--yellow)">-</div>
    </div>
    <div class="stat-card">
      <div class="label">Games Played</div>
      <div class="value" id="stat-games" style="color: var(--green)">-</div>
    </div>
    <div class="stat-card">
      <div class="label">Games / min</div>
      <div class="value" id="stat-rate" style="color: var(--text-dim)">-</div>
    </div>
  </div>

  <div class="chart-container">
    <h2>Elo Timeline</h2>
    <canvas id="elo-chart"></canvas>
  </div>
</div>

<script>
  // Tab switching
  document.querySelectorAll('.tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
      tab.classList.add('active');
    });
  });

  // Poll status
  async function updateStatus() {
    try {
      const res = await fetch('/api/status');
      const data = await res.json();
      document.getElementById('stat-engines').textContent = data.engines_tracked;
      document.getElementById('stat-active').textContent = data.active_jobs;
      document.getElementById('stat-games').textContent = data.total_games_played.toLocaleString();
      document.getElementById('stat-rate').textContent = data.games_per_minute.toFixed(1);
    } catch(e) {
      document.getElementById('status-indicator').textContent = '● Disconnected';
      document.getElementById('status-indicator').style.color = 'var(--red)';
    }
  }

  // Elo chart drawing (canvas-based, no dependencies)
  function drawEloChart(canvas, dataPoints) {
    const ctx = canvas.getContext('2d');
    const w = canvas.width = canvas.offsetWidth * 2;
    const h = canvas.height = canvas.offsetHeight * 2;
    ctx.scale(2, 2);
    const cw = w/2, ch = h/2;
    const pad = { top: 20, right: 20, bottom: 40, left: 60 };

    ctx.clearRect(0, 0, cw, ch);

    if (!dataPoints || dataPoints.length === 0) {
      ctx.fillStyle = '#8b949e';
      ctx.font = '14px monospace';
      ctx.textAlign = 'center';
      ctx.fillText('No data yet — waiting for test results...', cw/2, ch/2);
      return;
    }

    const elos = dataPoints.map(d => d.elo);
    const minElo = Math.min(...elos) - 10;
    const maxElo = Math.max(...elos) + 10;
    const chartW = cw - pad.left - pad.right;
    const chartH = ch - pad.top - pad.bottom;

    // Grid lines
    ctx.strokeStyle = '#30363d';
    ctx.lineWidth = 0.5;
    for (let i = 0; i <= 5; i++) {
      const y = pad.top + (chartH * i / 5);
      ctx.beginPath();
      ctx.moveTo(pad.left, y);
      ctx.lineTo(cw - pad.right, y);
      ctx.stroke();
    }

    // Zero line
    if (minElo < 0 && maxElo > 0) {
      const zeroY = pad.top + chartH * (1 - (0 - minElo) / (maxElo - minElo));
      ctx.strokeStyle = '#58a6ff44';
      ctx.lineWidth = 1;
      ctx.setLineDash([4, 4]);
      ctx.beginPath();
      ctx.moveTo(pad.left, zeroY);
      ctx.lineTo(cw - pad.right, zeroY);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Line chart
    ctx.strokeStyle = '#58a6ff';
    ctx.lineWidth = 2;
    ctx.beginPath();
    dataPoints.forEach((d, i) => {
      const x = pad.left + (chartW * i / (dataPoints.length - 1));
      const y = pad.top + chartH * (1 - (d.elo - minElo) / (maxElo - minElo));
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    });
    ctx.stroke();

    // Data points
    dataPoints.forEach((d, i) => {
      const x = pad.left + (chartW * i / (dataPoints.length - 1));
      const y = pad.top + chartH * (1 - (d.elo - minElo) / (maxElo - minElo));
      ctx.fillStyle = d.is_release ? '#d29922' : (d.tag ? '#3fb950' : '#58a6ff');
      ctx.beginPath();
      ctx.arc(x, y, d.is_release ? 5 : 3, 0, Math.PI * 2);
      ctx.fill();
    });

    // Y-axis labels
    ctx.fillStyle = '#8b949e';
    ctx.font = '11px monospace';
    ctx.textAlign = 'right';
    for (let i = 0; i <= 5; i++) {
      const elo = minElo + (maxElo - minElo) * (1 - i / 5);
      const y = pad.top + (chartH * i / 5);
      ctx.fillText(elo.toFixed(0), pad.left - 8, y + 4);
    }
  }

  // Init
  updateStatus();
  setInterval(updateStatus, 3000);

  // Draw empty chart initially
  const canvas = document.getElementById('elo-chart');
  if (canvas) {
    setTimeout(() => drawEloChart(canvas, []), 100);
  }
</script>
</body>
</html>
"##;

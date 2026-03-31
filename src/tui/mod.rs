//! Terminal UI for monitoring Crucible in real-time.
//!
//! Shows live game progress, SPRT status, Elo timeline,
//! and job queue — all in the terminal.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    prelude::*,
    widgets::*,
};
use std::io::stdout;
use std::time::Duration;

use crate::storage::Storage;
use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Dashboard,
    Jobs,
    Timeline,
    Bisect,
}

pub struct Tui {
    storage: Storage,
    current_tab: Tab,
}

impl Tui {
    pub fn new(storage: Storage) -> Self {
        Self {
            storage,
            current_tab: Tab::Dashboard,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        stdout().execute(EnterAlternateScreen)?;
        enable_raw_mode()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

        loop {
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Char('1') => self.current_tab = Tab::Dashboard,
                            KeyCode::Char('2') => self.current_tab = Tab::Jobs,
                            KeyCode::Char('3') => self.current_tab = Tab::Timeline,
                            KeyCode::Char('4') => self.current_tab = Tab::Bisect,
                            KeyCode::Tab => {
                                self.current_tab = match self.current_tab {
                                    Tab::Dashboard => Tab::Jobs,
                                    Tab::Jobs => Tab::Timeline,
                                    Tab::Timeline => Tab::Bisect,
                                    Tab::Bisect => Tab::Dashboard,
                                };
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        disable_raw_mode()?;
        stdout().execute(LeaveAlternateScreen)?;
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Title bar
                Constraint::Length(3),  // Tab bar
                Constraint::Min(0),    // Content
                Constraint::Length(1), // Status bar
            ])
            .split(frame.area());

        // Title
        let title = Paragraph::new("⚗  CRUCIBLE — Chess Engine CI")
            .style(Style::default().fg(Color::Cyan).bold())
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(title, chunks[0]);

        // Tab bar
        let tabs = Tabs::new(vec!["[1] Dashboard", "[2] Jobs", "[3] Timeline", "[4] Bisect"])
            .select(match self.current_tab {
                Tab::Dashboard => 0,
                Tab::Jobs => 1,
                Tab::Timeline => 2,
                Tab::Bisect => 3,
            })
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(Style::default().fg(Color::Yellow).bold());
        frame.render_widget(tabs, chunks[1]);

        // Content
        match self.current_tab {
            Tab::Dashboard => self.draw_dashboard(frame, chunks[2]),
            Tab::Jobs => self.draw_jobs(frame, chunks[2]),
            Tab::Timeline => self.draw_timeline(frame, chunks[2]),
            Tab::Bisect => self.draw_bisect(frame, chunks[2]),
        }

        // Status bar
        let status = Paragraph::new(" q: Quit │ Tab: Switch tab │ 1-4: Jump to tab")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[3]);
    }

    fn draw_dashboard(&self, frame: &mut Frame, area: Rect) {
        let status = self.storage.get_system_status().unwrap_or(SystemStatus {
            active_jobs: 0,
            queued_jobs: 0,
            completed_jobs: 0,
            engines_tracked: 0,
            total_games_played: 0,
            uptime_seconds: 0,
            games_per_minute: 0.0,
        });

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Left: Stats
        let stats_text = vec![
            Line::from(vec![
                Span::raw("  Engines tracked:  "),
                Span::styled(status.engines_tracked.to_string(), Style::default().fg(Color::Green).bold()),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  Active jobs:      "),
                Span::styled(status.active_jobs.to_string(), Style::default().fg(Color::Yellow).bold()),
            ]),
            Line::from(vec![
                Span::raw("  Queued jobs:      "),
                Span::styled(status.queued_jobs.to_string(), Style::default().fg(Color::Blue)),
            ]),
            Line::from(vec![
                Span::raw("  Completed jobs:   "),
                Span::styled(status.completed_jobs.to_string(), Style::default().fg(Color::Green)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  Total games:      "),
                Span::styled(status.total_games_played.to_string(), Style::default().fg(Color::Cyan).bold()),
            ]),
            Line::from(vec![
                Span::raw("  Games/min:        "),
                Span::styled(format!("{:.1}", status.games_per_minute), Style::default().fg(Color::Cyan)),
            ]),
        ];

        let stats = Paragraph::new(stats_text)
            .block(Block::default().title(" System ").borders(Borders::ALL));
        frame.render_widget(stats, chunks[0]);

        // Right: Live game feed
        let live = Paragraph::new("  Waiting for games...")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().title(" Live Feed ").borders(Borders::ALL));
        frame.render_widget(live, chunks[1]);
    }

    fn draw_jobs(&self, frame: &mut Frame, area: Rect) {
        let header = Row::new(vec!["Status", "Engine", "Dev", "Base", "W/D/L", "Elo", "SPRT"])
            .style(Style::default().fg(Color::Yellow).bold());

        let table = Table::new(
            Vec::<Row>::new(), // Populated from storage in real implementation
            [
                Constraint::Length(10),
                Constraint::Length(15),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(15),
                Constraint::Length(12),
                Constraint::Length(14),
            ],
        )
        .header(header)
        .block(Block::default().title(" Test Jobs ").borders(Borders::ALL));

        frame.render_widget(table, area);
    }

    fn draw_timeline(&self, frame: &mut Frame, area: Rect) {
        // ASCII Elo chart
        let chart_block = Block::default().title(" Elo Timeline ").borders(Borders::ALL);
        let inner = chart_block.inner(area);
        frame.render_widget(chart_block, area);

        // Placeholder - in real implementation, draw sparkline/chart from Elo data points
        let placeholder = Paragraph::new("  Elo timeline chart will render here\n  (commits along X axis, Elo on Y axis)")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(placeholder, inner);
    }

    fn draw_bisect(&self, frame: &mut Frame, area: Rect) {
        let bisect_info = Paragraph::new(vec![
            Line::from("  No active bisect sessions."),
            Line::from(""),
            Line::from("  Start a bisect with:"),
            Line::from(Span::styled(
                "    crucible bisect --engine <name> --good <commit> --bad <commit>",
                Style::default().fg(Color::Green),
            )),
        ])
        .block(Block::default().title(" Bisect ").borders(Borders::ALL));
        frame.render_widget(bisect_info, area);
    }
}

use std::{
    fs,
    io,
    process::Command,
    time::Duration,
    path::PathBuf,
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, Tabs},
    Terminal, Frame,
};
use serde::Deserialize;
use dirs;

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_backend")]
    backend: String,
}

fn default_backend() -> String {
    "pw".to_string()
}

#[derive(Debug, Clone)]
struct Sink {
    name: String,
    streams: Vec<Stream>,
}

#[derive(Debug, Clone)]
struct Stream {
    id: String,
    name: String,
}

fn load_config() -> Config {
    let mut path = dirs::config_dir().unwrap_or(PathBuf::from("."));
    path.push("marstui/sink.toml");
    let content = fs::read_to_string(path).expect("Could not read sink.toml");
    toml::from_str(&content).expect("Could not parse sink.toml")
}

fn fetch_sinks(config: &Config) -> Vec<Sink> {
    if config.backend == "pw" {
        let output = Command::new("pw-cli")
            .arg("list-objects")
            .output()
            .expect("failed to run pw-cli");

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .filter(|l| l.contains("PipeWire:Interface:Node") && l.contains("Audio/Sink"))
            .map(|l| Sink {
                name: l.trim().to_string(),
                streams: vec![],
            })
            .collect()
    } else {
        let output = Command::new("pactl")
            .arg("list")
            .arg("sinks")
            .output()
            .expect("failed to run pactl");

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .split("Sink #")
            .skip(1)
            .map(|s| {
                let name_line = s
                    .lines()
                    .find(|l| l.trim().starts_with("Name:"))
                    .unwrap_or("Name: unknown");
                let name = name_line.split_whitespace().nth(1).unwrap_or("unknown").to_string();
                Sink { name, streams: vec![] }
            })
            .collect()
    }
}

fn fetch_streams(config: &Config, sink: &Sink) -> Vec<Stream> {
    if config.backend == "pw" {
        let output = Command::new("pw-cli")
            .arg("list-objects")
            .output()
            .expect("failed to run pw-cli");

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .filter(|l| l.contains("PipeWire:Interface:Node") && l.contains("Audio/Stream"))
            .map(|l| Stream {
                id: l.to_string(),
                name: l.to_string(),
            })
            .collect()
    } else {
        let output = Command::new("pactl")
            .arg("list")
            .arg("sink-inputs")
            .output()
            .expect("failed to run pactl");

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .split("Sink Input #")
            .skip(1)
            .map(|s| {
                let id = s.lines().next().unwrap_or("0").trim().to_string();
                let name_line = s
                    .lines()
                    .find(|l| l.trim().starts_with("application.name ="))
                    .unwrap_or("application.name = \"unknown\"");
                let name = name_line
                    .split('=')
                    .nth(1)
                    .unwrap_or("\"unknown\"")
                    .trim()
                    .trim_matches('"')
                    .to_string();
                Stream { id, name }
            })
            .collect()
    }
}

fn attach_stream(config: &Config, stream: &Stream, sink: &Sink) {
    if config.backend == "pw" {
        let _ = Command::new("pw-link")
            .arg(&stream.id)
            .arg(&sink.name)
            .status();
    } else {
        let _ = Command::new("pactl")
            .arg("move-sink-input")
            .arg(&stream.id)
            .arg(&sink.name)
            .status();
    }
}

fn detach_stream(config: &Config, stream: &Stream, sink: &Sink) {
    if config.backend == "pw" {
        let _ = Command::new("pw-link")
            .arg("-d")
            .arg(&stream.id)
            .arg(&sink.name)
            .status();
    } else {
        let _ = Command::new("pactl")
            .arg("suspend-sink-input")
            .arg(&stream.id)
            .arg("1")
            .status();
    }
}

/// Draws the full UI
fn draw_ui(f: &mut Frame<CrosstermBackend<io::Stdout>>, config: &Config, sinks: &Vec<Sink>, selected_sink: usize, selected_action: usize, actions: &Vec<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.size());

    // Top bar
    let top = Block::default()
        .title(format!(
            " Backend: {} | Selected sink: {} ",
            config.backend,
            sinks.get(selected_sink).map(|s| &s.name).unwrap_or(&"-".to_string())
        ))
        .borders(Borders::ALL);
    f.render_widget(top, chunks[0]);

    // Sink list
    let items: Vec<ListItem> = sinks
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == selected_sink {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            ListItem::new(Spans::from(Span::styled(&s.name, style)))
        })
        .collect();
    let sink_list = List::new(items).block(Block::default().borders(Borders::ALL).title("Sinks"));
    f.render_widget(sink_list, chunks[1]);

    // Bottom bar
    let spans: Vec<Spans> = actions
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let style = if i == selected_action {
                Style::default().fg(Color::Yellow).bg(Color::Blue)
            } else {
                Style::default()
            };
            Spans::from(Span::styled(*t, style))
        })
        .collect();
    let tabs = Tabs::new(spans)
        .block(Block::default().borders(Borders::ALL).title("Actions"));
    f.render_widget(tabs, chunks[2]);
}

fn main() -> Result<(), io::Error> {
    let config = load_config();
    let mut sinks = fetch_sinks(&config);
    let mut selected_sink = 0usize;
    let mut selected_action = 0usize;
    let mut selected_stream = 0usize;
    let actions = vec!["Create", "Delete", "Modify"];

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut sink = Terminal::new(backend)?;

    loop {
        sink.draw(|f| {
            draw_ui(f, &config, &sinks, selected_sink, selected_action, &actions);
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Left => if selected_action > 0 { selected_action -= 1; },
                    KeyCode::Right => if selected_action < actions.len() - 1 { selected_action += 1; },
                    KeyCode::Up => if selected_sink > 0 { selected_sink -= 1; },
                    KeyCode::Down => if selected_sink < sinks.len().saturating_sub(1) { selected_sink += 1; },
                    KeyCode::Char('r') => { sinks = fetch_sinks(&config); }
                    KeyCode::Char('m') => {
                        if selected_action == 2 {
                            if let Some(sink) = sinks.get_mut(selected_sink) {
                                sink.streams = fetch_streams(&config, sink);
                            }
                        }
                    }
                    KeyCode::Char('a') => {
                        if selected_action == 2 {
                            if let Some(sink) = sinks.get(selected_sink) {
                                if let Some(stream) = sink.streams.get(selected_stream) {
                                    attach_stream(&config, stream, sink);
                                }
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        if selected_action == 2 {
                            if let Some(sink) = sinks.get(selected_sink) {
                                if let Some(stream) = sink.streams.get(selected_stream) {
                                    detach_stream(&config, stream, sink);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(sink.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    sink.show_cursor()?;
    Ok(())
}
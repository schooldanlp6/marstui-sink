use std::{
    fs,
    io,
    path::PathBuf,
    process::Command,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, Tabs},
    Terminal,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
struct Config {
    backend: String, // "pw" for PipeWire, "pl" for Pulse
}

impl Default for Config {
    fn default() -> Self {
        Self { backend: "pw".to_string() }
    }
}

fn load_config() -> Config {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("marstui/sink.toml");

    if !config_path.exists() {
        let default = Config::default();
        let toml_str = toml::to_string(&default).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, toml_str).unwrap();
        default
    } else {
        let content = fs::read_to_string(&config_path).unwrap();
        toml::from_str(&content).unwrap_or_else(|_| Config::default())
    }
}

#[derive(Clone)]
struct Sink {
    name: String,
    module_id: Option<u32>,
    streams: Vec<Stream>,
}

#[derive(Clone)]
struct Stream {
    id: u32,
    name: String,
}

enum Mode {
    Normal,
    Modify(usize),
}

enum ActiveList {
    Attached,
    Available,
}

fn run_pactl(args: &[&str]) -> String {
    String::from_utf8(
        Command::new("pactl").args(args).output().unwrap().stdout,
    )
    .unwrap()
}

fn fetch_sinks() -> Vec<Sink> {
    let output = run_pactl(&["list", "short", "sinks"]);
    let mut sinks = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[1].to_string();
            sinks.push(Sink {
                name,
                module_id: None, // we’ll fill for null sinks
                streams: vec![],
            });
        }
    }

    // attach streams
    let sinputs = run_pactl(&["list", "sink-inputs"]);
    let mut current_id = None;
    let mut current_name = String::new();
    let mut current_sink = String::new();

    for line in sinputs.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Sink Input #") {
            if let Some(id) = current_id {
                if let Some(sink) = sinks.iter_mut().find(|s| s.name == current_sink) {
                    sink.streams.push(Stream { id, name: current_name.clone() });
                }
            }
            current_id = trimmed.split('#').nth(1).and_then(|n| n.trim().parse::<u32>().ok());
            current_name.clear();
            current_sink.clear();
        } else if trimmed.starts_with("Sink:") {
            current_sink = trimmed.split_whitespace().nth(1).unwrap_or("").to_string();
        } else if trimmed.starts_with("application.name =") {
            current_name = trimmed.splitn(2, '=').nth(1).unwrap_or("").trim_matches('"').to_string();
        }
    }
    if let Some(id) = current_id {
        if let Some(sink) = sinks.iter_mut().find(|s| s.name == current_sink) {
            sink.streams.push(Stream { id, name: current_name.clone() });
        }
    }

    sinks
}

fn fetch_available_streams(sinks: &[Sink]) -> Vec<Stream> {
    let mut all = Vec::new();
    let sinputs = run_pactl(&["list", "sink-inputs"]);

    let mut current_id = None;
    let mut current_name = String::new();
    let mut current_sink = String::new();

    for line in sinputs.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Sink Input #") {
            if let Some(id) = current_id {
                let attached = sinks.iter().any(|s| s.streams.iter().any(|st| st.id == id));
                if !attached {
                    all.push(Stream { id, name: current_name.clone() });
                }
            }
            current_id = trimmed.split('#').nth(1).and_then(|n| n.trim().parse::<u32>().ok());
            current_name.clear();
            current_sink.clear();
        } else if trimmed.starts_with("application.name =") {
            current_name = trimmed.splitn(2, '=').nth(1).unwrap_or("").trim_matches('"').to_string();
        } else if trimmed.starts_with("Sink:") {
            current_sink = trimmed.split_whitespace().nth(1).unwrap_or("").to_string();
        }
    }
    if let Some(id) = current_id {
        let attached = sinks.iter().any(|s| s.streams.iter().any(|st| st.id == id));
        if !attached {
            all.push(Stream { id, name: current_name.clone() });
        }
    }

    all
}

fn create_sink() {
    let name = format!("marstui-null-{}", chrono::Utc::now().timestamp());
    let _ = Command::new("pactl")
        .args(&["load-module", "module-null-sink", &format!("sink_name={}", name)])
        .output();
}

fn delete_sink(name: &str) {
    // find its module
    let modules = run_pactl(&["list", "short", "modules"]);
    for line in modules.lines() {
        if line.contains(name) {
            if let Some(id) = line.split_whitespace().next() {
                let _ = Command::new("pactl").args(&["unload-module", id]).output();
            }
        }
    }
}

fn attach_stream(stream: &Stream, sink: &Sink) {
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream.id.to_string(), &sink.name])
        .output();
}

fn detach_stream(stream: &Stream) {
    // move to default sink
    let def = run_pactl(&["info"]);
    let default_sink = def.lines().find(|l| l.contains("Default Sink")).unwrap_or("Default Sink: @DEFAULT_SINK@");
    let sink = default_sink.split(':').nth(1).unwrap().trim();
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream.id.to_string(), sink])
        .output();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _config = load_config();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut sink = Terminal::new(backend)?;

    let mut sinks = fetch_sinks();
    let mut mode = Mode::Normal;
    let mut selected_sink = 0usize;
    let mut selected_action = 0usize;
    let mut active_list = ActiveList::Attached;
    let mut selected_attached = 0usize;
    let mut selected_available = 0usize;

    loop {
        let available_streams = fetch_available_streams(&sinks);

        sink.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(3)].as_ref())
                .split(f.size());

            let top = Block::default().borders(Borders::ALL).title("Selected Sink");
            let title = sinks.get(selected_sink).map(|s| s.name.clone()).unwrap_or_default();
            let paragraph = tui::widgets::Paragraph::new(title).block(top);
            f.render_widget(paragraph, chunks[0]);

            // Middle
            match mode {
                Mode::Normal => {
                    let items: Vec<ListItem> = sinks.iter().enumerate().map(|(i, s)| {
                        let style = if i == selected_sink { Style::default().fg(Color::Yellow) } else { Style::default() };
                        ListItem::new(Spans::from(Span::styled(s.name.clone(), style)))
                    }).collect();
                    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Sinks"));
                    f.render_widget(list, chunks[1]);
                }
                Mode::Modify(idx) => {
                    if let Some(s) = sinks.get(idx) {
                        let inner = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                            .split(chunks[1]);

                        let attached: Vec<ListItem> = s.streams.iter().enumerate().map(|(i, st)| {
                            let style = if matches!(active_list, ActiveList::Attached) && i == selected_attached {
                                Style::default().fg(Color::Black).bg(Color::Yellow)
                            } else { Style::default() };
                            ListItem::new(Spans::from(Span::styled(st.name.clone(), style)))
                        }).collect();
                        let available: Vec<ListItem> = available_streams.iter().enumerate().map(|(i, st)| {
                            let style = if matches!(active_list, ActiveList::Available) && i == selected_available {
                                Style::default().fg(Color::Black).bg(Color::Yellow)
                            } else { Style::default() };
                            ListItem::new(Spans::from(Span::styled(st.name.clone(), style)))
                        }).collect();

                        f.render_widget(List::new(attached).block(Block::default().borders(Borders::ALL).title("Attached")), inner[0]);
                        f.render_widget(List::new(available).block(Block::default().borders(Borders::ALL).title("Available")), inner[1]);
                    }
                }
            }

            // Bottom
            let bottom_titles = vec!["Create", "Delete", "Modify"];
            let spans: Vec<Spans> = bottom_titles.iter().enumerate().map(|(i, t)| {
                let style = if i == selected_action {
                    Style::default().fg(Color::Yellow).bg(Color::Blue)
                } else { Style::default() };
                Spans::from(Span::styled(*t, style))
            }).collect();
            let tabs = Tabs::new(spans).block(Block::default().borders(Borders::ALL).title("Actions"));
            f.render_widget(tabs, chunks[2]);
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                match mode {
                    Mode::Normal => match k.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Up => if selected_sink > 0 { selected_sink -= 1; },
                        KeyCode::Down => if selected_sink + 1 < sinks.len() { selected_sink += 1; },
                        KeyCode::Left => if selected_action > 0 { selected_action -= 1; },
                        KeyCode::Right => if selected_action < 2 { selected_action += 1; },
                        KeyCode::Enter => {
                            match selected_action {
                                0 => create_sink(),
                                1 => {
                                    if let Some(s) = sinks.get(selected_sink) {
                                        delete_sink(&s.name);
                                    }
                                }
                                2 => mode = Mode::Modify(selected_sink),
                                _ => {}
                            }
                        }
                        _ => {}
                    },
                    Mode::Modify(idx) => match k.code {
                        KeyCode::Char('q') => mode = Mode::Normal,
                        KeyCode::Tab => {
                            active_list = match active_list {
                                ActiveList::Attached => ActiveList::Available,
                                ActiveList::Available => ActiveList::Attached,
                            };
                        }
                        KeyCode::Up => match active_list {
                            ActiveList::Attached => if selected_attached > 0 { selected_attached -= 1; },
                            ActiveList::Available => if selected_available > 0 { selected_available -= 1; },
                        },
                        KeyCode::Down => match active_list {
                            ActiveList::Attached => selected_attached += 1,
                            ActiveList::Available => selected_available += 1,
                        },
                        KeyCode::Enter => {
                            if let Some(sink) = sinks.get(idx) {
                                match active_list {
                                    ActiveList::Attached => {
                                        if let Some(stream) = sink.streams.get(selected_attached) {
                                            detach_stream(stream);
                                        }
                                    }
                                    ActiveList::Available => {
                                        if let Some(stream) = available_streams.get(selected_available) {
                                            attach_stream(stream, sink);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                sinks = fetch_sinks();
            }
        }
    }

    disable_raw_mode()?;
    execute!(sink.backend_mut(), LeaveAlternateScreen)?;
    sink.show_cursor()?;
    Ok(())
}
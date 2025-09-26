use std::{process::Command, io};
use std::path::PathBuf;
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, Tabs, Paragraph},
    Terminal,
};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use serde::{Deserialize, Serialize};
use dirs;
use std::fs;
use chrono::Utc;

// -------------------- CONFIG --------------------

#[derive(Deserialize, Serialize, Debug)]
struct Config {
    quit_key: char,
    backend: String, // "pw" for PipeWire, "pl" for PulseAudio
    top_fg: String,
    top_bg: String,
    bottom_fg: String,
    bottom_bg: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            quit_key: 'q',
            backend: "pw".to_string(),
            top_fg: "White".to_string(),
            top_bg: "Black".to_string(),
            bottom_fg: "Gray".to_string(),
            bottom_bg: "Black".to_string(),
        }
    }
}

fn load_config() -> Config {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("marstui/sink.toml");

    if !config_path.exists() {
        let default_config = Config::default();
        let config_toml = toml::to_string(&default_config).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).expect("Failed to create config directory");
        fs::write(&config_path, config_toml).expect("Failed to write default config file");
        default_config
    } else {
        let config_content = fs::read_to_string(&config_path).expect("Failed to read config file");
        toml::from_str(&config_content).unwrap_or_default()
    }
}

// -------------------- DATA STRUCTURES --------------------

#[derive(Clone)]
struct Stream {
    id: u32,
    name: String,
}

#[derive(Clone)]
struct Sink {
    name: String,
    module_id: u32,
    streams: Vec<Stream>,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Modify(usize),
}

#[derive(PartialEq)]
enum ActiveList {
    Attached,
    Available,
}

// -------------------- BACKEND FUNCTIONS --------------------

fn fetch_sinks(backend: &str) -> Vec<Sink> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sinks"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut sinks = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[1].to_string();
            let module_id = parts[0].parse::<u32>().unwrap_or(0);
            sinks.push(Sink {
                name: name.clone(),
                module_id,
                streams: fetch_sink_streams(&name, backend),
            });
        }
    }
    sinks
}

fn fetch_sink_streams(sink_name: &str, backend: &str) -> Vec<Stream> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout.lines().filter_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { return None; }
        let id = parts[0].parse::<u32>().ok()?;
        let name = parts[1].to_string();

        // Check if attached to this sink
        let sink_info = Command::new("pactl").args(&["list", "sink-inputs"]).output().unwrap();
        let sink_stdout = String::from_utf8_lossy(&sink_info.stdout);
        if sink_stdout.contains(&format!("Sink: {}", sink_name)) {
            Some(Stream { id, name })
        } else {
            None
        }
    }).collect()
}

fn fetch_available_streams(attached: &[Stream]) -> Vec<Stream> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout.lines().filter_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { return None; }
        let id = parts[0].parse::<u32>().ok()?;
        let name = parts[1].to_string();
        if attached.iter().any(|s| s.id == id) { None } else { Some(Stream { id, name }) }
    }).collect()
}

fn attach_stream(stream_id: u32, sink_name: &str, backend: &str) {
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream_id.to_string(), sink_name])
        .output();
}

fn detach_stream(stream_id: u32, backend: &str) {
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream_id.to_string(), "@DEFAULT_SINK@"])
        .output();
}

fn create_sink(backend: &str) {
    let name = format!("marstui-null-{}", Utc::now().timestamp());
    let _ = Command::new("pactl")
        .args(&["load-module", "module-null-sink", &format!("sink_name={}", name)])
        .output();
}

fn delete_sink(module_id: u32, name: &str, backend: &str) {
    if name.starts_with("marstui-null-") {
        let _ = Command::new("pactl")
            .args(&["unload-module", &module_id.to_string()])
            .output();
    }
}

// -------------------- MAIN --------------------

fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let config = load_config();
    let mut sinks = fetch_sinks(&config.backend);
    let mut selected_sink = 0;
    let mut mode = Mode::Normal;
    let mut active_list = ActiveList::Attached;
    let mut selected_attached = 0;
    let mut selected_available = 0;
    let mut selected_action = 0;

    loop {
        terminal.draw(|f| {
            let size = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)].as_ref())
                .split(size);

            // --- Top bar ---
            let top_bar = Paragraph::new(Spans::from(vec![Span::raw(
                format!("Selected Sink: {}", sinks.get(selected_sink).map(|s| &s.name).unwrap_or(&"<none>".to_string()))
            )]))
            .block(Block::default().borders(Borders::ALL).title("Sink Manager"));
            f.render_widget(top_bar, chunks[0]);

            // --- Middle area ---
            match mode {
                Mode::Normal => {
                    let items: Vec<ListItem> = sinks.iter().enumerate().map(|(i,s)| {
                        let style = if i == selected_sink { Style::default().fg(Color::Yellow) } else { Style::default() };
                        ListItem::new(Span::styled(&s.name, style))
                    }).collect();
                    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Sinks"));
                    f.render_widget(list, chunks[1]);
                }
                Mode::Modify(idx) => {
                    if let Some(sink) = sinks.get(idx) {
                        let half = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                            .split(chunks[1]);

                        // Attached streams
                        let attached_items: Vec<ListItem> = sink.streams.iter().enumerate().map(|(i, stream)| {
                            let style = if active_list==ActiveList::Attached && i==selected_attached { Style::default().fg(Color::Yellow) } else { Style::default() };
                            ListItem::new(Span::styled(format!("{} [detach]", stream.name), style))
                        }).collect();
                        let attached_list = List::new(attached_items).block(Block::default().borders(Borders::ALL).title("Attached Streams"));
                        f.render_widget(attached_list, half[0]);

                        // Available streams
                        let available = fetch_available_streams(&sink.streams);
                        let available_items: Vec<ListItem> = available.iter().enumerate().map(|(i, stream)| {
                            let style = if active_list==ActiveList::Available && i==selected_available { Style::default().fg(Color::Yellow) } else { Style::default() };
                            ListItem::new(Span::styled(format!("{} [attach]", stream.name), style))
                        }).collect();
                        let available_list = List::new(available_items).block(Block::default().borders(Borders::ALL).title("Available Streams"));
                        f.render_widget(available_list, half[1]);
                    }
                }
            }

            // --- Bottom bar ---
            let bottom_titles = vec!["Create", "Delete", "Modify"];
            let spans: Vec<Spans> = bottom_titles.iter().enumerate().map(|(i, t)| {
                let style = if i == selected_action { Style::default().fg(Color::Yellow).bg(Color::Blue) } else { Style::default() };
                Spans::from(Span::styled(*t, style))
            }).collect();
            let tabs = Tabs::new(spans).block(Block::default().borders(Borders::ALL).title("Actions"));
            f.render_widget(tabs, chunks[2]);
        })?;

        // --- Event loop ---
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match mode {
                    Mode::Normal => match key.code {
                        KeyCode::Char(c) if c == config.quit_key => break,
                        KeyCode::Up => if selected_sink > 0 { selected_sink -= 1 },
                        KeyCode::Down => if selected_sink + 1 < sinks.len() { selected_sink += 1 },
                        KeyCode::Left => if selected_action > 0 { selected_action -= 1 },
                        KeyCode::Right => if selected_action < 2 { selected_action += 1 },
                        KeyCode::Enter => match selected_action {
                            0 => { create_sink(&config.backend); sinks = fetch_sinks(&config.backend); },
                            1 => { if let Some(s) = sinks.get(selected_sink) { delete_sink(s.module_id, &s.name, &config.backend); sinks = fetch_sinks(&config.backend); if selected_sink>=sinks.len() && selected_sink>0 { selected_sink-=1; } } },
                            2 => mode = Mode::Modify(selected_sink),
                            _ => {}
                        },
                        _ => {}
                    },
                    Mode::Modify(idx) => {
                        if let Some(sink) = sinks.get(idx) {
                            let attached_len = sink.streams.len();
                            let available = fetch_available_streams(&sink.streams);
                            let available_len = available.len();

                            match key.code {
                                KeyCode::Char(c) if c == config.quit_key => break,
                                KeyCode::Tab => active_list = if active_list==ActiveList::Attached { ActiveList::Available } else { ActiveList::Attached },
                                KeyCode::Up => match active_list {
                                    ActiveList::Attached => if selected_attached>0 { selected_attached-=1 },
                                    ActiveList::Available => if selected_available>0 { selected_available-=1 },
                                },
                                KeyCode::Down => match active_list {
                                    ActiveList::Attached => if selected_attached+1 < attached_len { selected_attached+=1 },
                                    ActiveList::Available => if selected_available+1 < available_len { selected_available+=1 },
                                },
                                KeyCode::Enter => match active_list {
                                    ActiveList::Attached => {
                                        if let Some(stream) = sink.streams.get(selected_attached) {
                                            detach_stream(stream.id, &config.backend);
                                            sinks = fetch_sinks(&config.backend);
                                        }
                                    },
                                    ActiveList::Available => {
                                        if let Some(stream) = available.get(selected_available) {
                                            attach_stream(stream.id, &sink.name, &config.backend);
                                            sinks = fetch_sinks(&config.backend);
                                        }
                                    }
                                },
                                KeyCode::Esc => {
                                    mode = Mode::Normal;
                                    selected_attached = 0;
                                    selected_available = 0;
                                    active_list = ActiveList::Attached;
                                },
                                _ => {}
                            }
                        } else {
                            mode = Mode::Normal;
                        }
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
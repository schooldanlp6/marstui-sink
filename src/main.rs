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
use chrono::Utc;

#[derive(Deserialize, Serialize, Debug)]
struct Config {
    backend: String,        // "pw" or "pl" for fetching sinks
    attach_backend: String, // "pw" or "pl" for attach/detach
}

impl Default for Config {
    fn default() -> Self {
        Self { 
            backend: "pw".to_string(),
            attach_backend: "pl".to_string(), // default to PulseAudio for safety
        }
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

enum Focus {
    Streams,
    Sinks,
}

fn run_cmd(cmd: &str, args: &[&str]) -> String {
    String::from_utf8(Command::new(cmd).args(args).output().unwrap().stdout).unwrap()
}

fn fetch_sinks(config: &Config) -> Vec<Sink> {
    match config.backend.as_str() {
        "pl" => fetch_sinks_pulse(),
        "pw" => fetch_sinks_pipewire(),
        _ => vec![],
    }
}

fn fetch_sinks_pulse() -> Vec<Sink> {
    let output = run_cmd("pactl", &["list", "short", "sinks"]);
    let mut sinks = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            sinks.push(Sink {
                name: parts[1].to_string(),
                module_id: None,
                streams: vec![],
            });
        }
    }

    // attach streams
    let sinputs = run_cmd("pactl", &["list", "sink-inputs"]);
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

// TODO: implement PipeWire backend if needed
fn fetch_sinks_pipewire() -> Vec<Sink> {
    // placeholder: for now behave same as PulseAudio if pw not installed
    fetch_sinks_pulse()
}

fn fetch_available_streams(sinks: &[Sink], config: &Config) -> Vec<Stream> {
    match config.backend.as_str() {
        "pl" => fetch_available_streams_pulse(sinks),
        "pw" => fetch_available_streams_pulse(sinks),
        _ => vec![],
    }
}

fn fetch_available_streams_pulse(sinks: &[Sink]) -> Vec<Stream> {
    let mut all = Vec::new();
    let sinputs = run_cmd("pactl", &["list", "sink-inputs"]);

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
    let name = format!("marstui-null-{}", Utc::now().timestamp());
    let _ = Command::new("pactl")
        .args(&["load-module", "module-null-sink", &format!("sink_name={}", name)])
        .output();
}

fn delete_sink(name: &str) {
    let modules = run_cmd("pactl", &["list", "short", "modules"]);
    for line in modules.lines() {
        if line.contains(name) {
            if let Some(id) = line.split_whitespace().next() {
                let _ = Command::new("pactl").args(&["unload-module", id]).output();
            }
        }
    }
}

fn attach_sink(config: &Config, stream: &Stream, sink: &Sink) {
    match config.attach_backend.as_str() {
        "pl" => {
            // works on PulseAudio and PipeWire with pactl compatibility
            let _ = Command::new("pactl")
                .args(&["move-sink-input", &stream.id.to_string(), &sink.name])
                .output();
        }
        "pw" => {
            // placeholder: actual PipeWire native attach requires pw-dump / port IDs
            eprintln!("PipeWire native attach not yet implemented");
        }
        _ => {}
    }
}

fn detach_sink(config: &Config, stream: &Stream) {
    match config.attach_backend.as_str() {
        "pl" => {
            // move back to default sink
            let def = run_cmd("pactl", &["info"]);
            let default_sink = def.lines()
                .find(|l| l.contains("Default Sink"))
                .unwrap_or("Default Sink: @DEFAULT_SINK@")
                .split(':').nth(1).unwrap().trim();
            let _ = Command::new("pactl")
                .args(&["move-sink-input", &stream.id.to_string(), default_sink])
                .output();
        }
        "pw" => {
            eprintln!("PipeWire native detach not yet implemented");
        }
        _ => {}
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut sink = Terminal::new(backend)?;

    let mut sinks = fetch_sinks(&config);
    let mut mode = Mode::Normal;
    let mut selected_sink = 0;
    let mut selected_action = 0;
    let mut focus = Focus::Streams;
    let mut selected_stream = 0;

    loop {
        let available_streams = fetch_available_streams(&sinks, &config);
        sinks = fetch_sinks(&config);
        let attached_streams = &sinks[selected_sink].streams;
        let available_streams = fetch_available_streams(&sinks, &config);
        
        // Compute once for this loop iteration
        let all_streams: Vec<(Stream, bool)> = attached_streams
            .iter()
            .map(|s| (s.clone(), true))
            .chain(available_streams.into_iter().map(|s| {
                let attached = attached_streams.iter().any(|a| a.id == s.id);
                (s, attached)
            }))
            .collect();

        sink.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(3)].as_ref())
                .split(f.size());

            // Top
            let top_title = sinks.get(selected_sink).map(|s| s.name.clone()).unwrap_or_default();
            let top = Block::default().borders(Borders::ALL).title("Selected Sink");
            f.render_widget(tui::widgets::Paragraph::new(top_title).block(top), chunks[0]);

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
                    let inner = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                        .split(chunks[1]);

                    // Fetch latest streams each iteration
                    let attached_streams = &sinks[selected_sink].streams;
                    let available_streams = fetch_available_streams(&sinks, &config);

                    // Combine attached and available streams into one vector with attached marker
                    let all_streams: Vec<(Stream, bool)> = attached_streams
                        .iter()
                        .map(|s| (s.clone(), true))
                        .chain(
                            available_streams.into_iter().map(|s| {
                                let attached = attached_streams.iter().any(|a| a.id == s.id);
                                (s, attached)
                            })
                        )
                        .collect();
                    
                    // Left = Streams
                    let stream_items: Vec<ListItem> = all_streams.iter().enumerate().map(|(i, (s, attached))| {
                        let label = format!("{:<50} {}", s.name, if *attached { "●" } else { "○" });
                        let style = if matches!(focus, Focus::Streams) && i == selected_stream {
                            Style::default().fg(Color::Black).bg(Color::Yellow)
                        } else { Style::default() };
                        ListItem::new(Spans::from(Span::styled(label, style)))
                    }).collect();
                
                    // Right = Sinks
                    let sink_items: Vec<ListItem> = sinks.iter().enumerate().map(|(i, s)| {
                        let style = if matches!(focus, Focus::Sinks) && i == selected_sink {
                            Style::default().fg(Color::Black).bg(Color::Yellow)
                        } else { Style::default() };
                        ListItem::new(Spans::from(Span::styled(s.name.clone(), style)))
                    }).collect();
                
                    f.render_widget(
                        List::new(stream_items)
                            .block(Block::default().borders(Borders::ALL).title("Streams")),
                        inner[0]
                    );
                
                    f.render_widget(
                        List::new(sink_items)
                            .block(Block::default().borders(Borders::ALL).title("Sinks")),
                        inner[1]
                    );
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
                        KeyCode::Up => if selected_sink > 0 { selected_sink -= 1 },
                        KeyCode::Down => if selected_sink + 1 < sinks.len() { selected_sink += 1 },
                        KeyCode::Left => if selected_action > 0 { selected_action -= 1 },
                        KeyCode::Right => if selected_action < 2 { selected_action += 1 },
                        KeyCode::Enter => match selected_action {
                            0 => create_sink(),
                            1 => if let Some(s) = sinks.get(selected_sink) {
                                delete_sink(&s.name);

                                if selected_sink > 0 {
                                    selected_sink -= 1;
                                } else if sinks.len() > 1 {
                                    selected_sink = sinks.len() - 1;
                                }
                            },
                            2 => mode = Mode::Modify(selected_sink),
                            _ => {}
                        },
                        _ => {}
                    },
                    Mode::Modify(_) => match k.code {
                        KeyCode::Char('q') => mode = Mode::Normal,
                        KeyCode::Left => focus = Focus::Streams,
                        KeyCode::Right => focus = Focus::Sinks,
                        KeyCode::Up => match focus {
                            Focus::Streams => if selected_stream > 0 { selected_stream -= 1 },
                            Focus::Sinks => if selected_sink > 0 { selected_sink -= 1 },
                        },KeyCode::Down => match focus {
                            Focus::Streams => selected_stream = selected_stream.saturating_add(1),
                            Focus::Sinks => selected_sink = selected_sink.saturating_add(1),
                        },KeyCode::Enter | KeyCode::Char('a') => {
                            if let Some((stream, attached)) = all_streams.get(selected_stream) {
                                if let Some(sink) = sinks.get(selected_sink) {
                                    // Only attach if not already attached
                                    if !*attached {
                                        attach_sink(&config, stream, sink);
                                    }
                                }
                            }
                        },KeyCode::Char('d') => {
                            if let Some((stream, attached)) = all_streams.get(selected_stream) {
                                // Only detach if attached
                                if *attached {
                                    detach_sink(&config, stream);
                                }
                            }
                        },
                        _ => {}
                    },
                }
                sinks = fetch_sinks(&config);
            }
        }
    }

    disable_raw_mode()?;
    execute!(sink.backend_mut(), LeaveAlternateScreen)?;
    sink.show_cursor()?;
    Ok(())
}
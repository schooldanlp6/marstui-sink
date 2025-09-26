use std::{
    io,
    process::Command,
    time::Duration,
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, Tabs},
    Terminal,
};
use chrono::Utc;

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

struct App {
    sinks: Vec<Sink>,
    selected_sink: usize,
    bottom_selected: usize,
    mode: Mode,
    selected_stream: usize,
    stream_scroll: usize,
}

enum Mode {
    Normal,
    Modify(usize),
}

fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App {
        sinks: fetch_sinks(),
        selected_sink: 0,
        bottom_selected: 0,
        mode: Mode::Normal,
        selected_stream: 0,
        stream_scroll: 0,
    };

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("{:?}", err);
    }

    Ok(())
}

// ---------------------------- UI LOOP ----------------------------

fn run_app<B: tui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| {
            let size = f.size();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Length(3), // top bar
                        Constraint::Min(5),    // middle
                        Constraint::Length(3), // bottom
                    ]
                    .as_ref(),
                )
                .split(size);

            // Top bar
            let top_text = match app.mode {
                Mode::Normal => format!(
                    "Current Sink: {}",
                    app.sinks
                        .get(app.selected_sink)
                        .map(|s| s.name.clone())
                        .unwrap_or_default()
                ),
                Mode::Modify(idx) => format!(
                    "Modify Sink: {}",
                    app.sinks
                        .get(idx)
                        .map(|s| s.name.clone())
                        .unwrap_or_default()
                ),
            };
            let top = Block::default().title(top_text);
            f.render_widget(top, chunks[0]);

            // Middle
            match app.mode {
                Mode::Normal => {
                    let items: Vec<ListItem> = app
                        .sinks
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            let style = if i == app.selected_sink {
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            };
                            ListItem::new(Span::styled(s.name.clone(), style))
                        })
                        .collect();
                    let list = List::new(items)
                        .block(Block::default().title("Sinks").borders(Borders::ALL));
                    f.render_widget(list, chunks[1]);
                }
                Mode::Modify(idx) => {
                    if let Some(sink) = app.sinks.get(idx) {
                        // Split middle into two vertical chunks: attached / available
                        let mid_chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                            .split(chunks[1]);

                        // Attached streams
                        let attached_items: Vec<ListItem> = sink
                            .streams
                            .iter()
                            .enumerate()
                            .map(|(i, stream)| {
                                let style = if i == app.selected_stream {
                                    Style::default()
                                        .fg(Color::Green)
                                        .add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                };
                                ListItem::new(Span::styled(format!("{} [detach]", stream.name), style))
                            })
                            .collect();

                        let attached_list = List::new(attached_items)
                            .block(Block::default().title("Attached Streams").borders(Borders::ALL));
                        f.render_widget(attached_list, mid_chunks[0]);

                        // Available streams
                        let all_streams = fetch_all_streams();
                        let available_streams: Vec<Stream> = all_streams
                            .into_iter()
                            .filter(|s| !sink.streams.iter().any(|st| st.id == s.id))
                            .collect();

                        let available_items: Vec<ListItem> = available_streams
                            .iter()
                            .map(|stream| ListItem::new(format!("{} [attach]", stream.name)))
                            .collect();

                        let available_list = List::new(available_items)
                            .block(Block::default().title("Available Streams").borders(Borders::ALL));
                        f.render_widget(available_list, mid_chunks[1]);
                    }
                }
            }

            // Bottom bar
            let bottom_items = vec!["Create", "Delete", "Modify"];
            let spans: Vec<Spans> = bottom_items
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    if i == app.bottom_selected && matches!(app.mode, Mode::Normal) {
                        Spans::from(Span::styled(
                            *label,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        ))
                    } else {
                        Spans::from(Span::raw(*label))
                    }
                })
                .collect();
            let tabs = Tabs::new(spans)
                .block(Block::default().borders(Borders::ALL).title("Actions"));
            f.render_widget(tabs, chunks[2]);
        })?;

        // Event handling
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Down => match app.mode {
                        Mode::Normal => {
                            app.selected_sink = (app.selected_sink + 1) % app.sinks.len();
                        }
                        Mode::Modify(idx) => {
                            if let Some(sink) = app.sinks.get(idx) {
                                if app.selected_stream + 1 < sink.streams.len() {
                                    app.selected_stream += 1;
                                }
                            }
                        }
                    },
                    KeyCode::Up => match app.mode {
                        Mode::Normal => {
                            if app.selected_sink == 0 {
                                app.selected_sink = app.sinks.len() - 1;
                            } else {
                                app.selected_sink -= 1;
                            }
                        }
                        Mode::Modify(idx) => {
                            if app.selected_stream > 0 {
                                app.selected_stream -= 1;
                            }
                        }
                    },
                    KeyCode::Left => {
                        if let Mode::Normal = app.mode {
                            if app.bottom_selected == 0 {
                                app.bottom_selected = 2;
                            } else {
                                app.bottom_selected -= 1;
                            }
                        }
                    }
                    KeyCode::Right => {
                        if let Mode::Normal = app.mode {
                            app.bottom_selected = (app.bottom_selected + 1) % 3;
                        }
                    }
                    KeyCode::Enter => match app.mode {
                        Mode::Normal => match app.bottom_selected {
                            0 => { // Create
                                let name = format!("marstui-null-{}", Utc::now().timestamp());
                                create_sink(&name);
                                app.sinks = fetch_sinks();
                            }
                            1 => { // Delete
                                if let Some(sink) = app.sinks.get(app.selected_sink) {
                                    if let Some(id) = sink.module_id {
                                        delete_sink(id);
                                        app.sinks = fetch_sinks();
                                    }
                                }
                            }
                            2 => { // Modify
                                let idx = app.selected_sink;
                                let mut sinks = fetch_sinks();
                                let streams = fetch_sink_streams(&sinks[idx].name);
                                sinks[idx].streams = streams;
                                app.sinks = sinks;
                                app.selected_stream = 0;
                                app.stream_scroll = 0;
                                app.mode = Mode::Modify(idx);
                            }
                            _ => {}
                        },
                        Mode::Modify(idx) => {
                            if let Some(sink) = app.sinks.get(idx) {
                                if app.selected_stream < sink.streams.len() {
                                    let stream_id = sink.streams[app.selected_stream].id;
                                    detach_stream(stream_id);
                                    let mut sinks = fetch_sinks();
                                    sinks[idx].streams = fetch_sink_streams(&sinks[idx].name);
                                    app.sinks = sinks;
                                }
                            }
                        }
                    },
                    KeyCode::Esc => app.mode = Mode::Normal,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------- PIPEWIRE COMMANDS ----------------------------

fn fetch_sinks() -> Vec<Sink> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sinks"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut sinks = vec![];
    for line in stdout.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() > 1 {
            let module_id = parts[0].parse::<u32>().ok();
            let name = parts[1].to_string();
            sinks.push(Sink {
                name: name.clone(),
                module_id,
                streams: fetch_sink_streams(&name),
            });
        }
    }
    sinks
}

fn fetch_sink_streams(sink_name: &str) -> Vec<Stream> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut streams = vec![];
    for line in stdout.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() > 1 {
            let id = parts[0].parse::<u32>().unwrap_or(0);
            let name = parts[1].to_string();
            if line.contains(sink_name) {
                streams.push(Stream { id, name });
            }
        }
    }
    streams
}

fn fetch_all_streams() -> Vec<Stream> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut streams = vec![];
    for line in stdout.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() > 1 {
            let id = parts[0].parse::<u32>().unwrap_or(0);
            let name = parts[1].to_string();
            streams.push(Stream { id, name });
        }
    }
    streams
}

fn create_sink(name: &str) {
    let _ = Command::new("pactl")
        .args(&["load-module", "module-null-sink", &format!("sink_name={}", name)])
        .output();
}

fn delete_sink(module_id: u32) {
    let _ = Command::new("pactl")
        .args(&["unload-module", &module_id.to_string()])
        .output();
}

fn detach_stream(stream_id: u32) {
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream_id.to_string(), "@DEFAULT_SINK@"])
        .output();
}

fn attach_stream(stream_id: u32, sink_name: &str) {
    let _ = Command::new("pactl")
        .args(&["move-sink-input", &stream_id.to_string(), sink_name])
        .output();
}
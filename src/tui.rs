use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bytes::Bytes;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tokio::sync::mpsc::{Sender, channel};
use tokio::task::spawn_blocking;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::config::RoleSpec;
use crate::features::{FeatureList, StatusCounts};
use crate::runner::{self, RunConfig};

#[derive(Debug, Clone, Copy)]
struct Size {
    cols: u16,
    rows: u16,
}

struct PtyPane {
    parser: Arc<RwLock<vt100::Parser>>,
    sender: Sender<Bytes>,
    master_pty: Box<dyn MasterPty>,
    exited: Arc<AtomicBool>,
    feature_id: Option<String>,
    agent_id: String,
}

impl PtyPane {
    fn new(
        size: Size,
        cmd: CommandBuilder,
        feature_id: Option<String>,
        agent_id: String,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: size.rows.saturating_sub(2),
                cols: size.cols.saturating_sub(2),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let parser = Arc::new(RwLock::new(vt100::Parser::new(
            size.rows.saturating_sub(2),
            size.cols.saturating_sub(2),
            0,
        )));
        let exited = Arc::new(AtomicBool::new(false));

        // Spawn the child process on the slave side of the PTY
        {
            let exited_clone = exited.clone();
            spawn_blocking(move || {
                match pty_pair.slave.spawn_command(cmd) {
                    Ok(mut child) => {
                        let _ = child.wait();
                    }
                    Err(_) => {}
                }
                exited_clone.store(true, Ordering::Relaxed);
                drop(pty_pair.slave);
            });
        }

        // Read PTY output and feed it to vt100::Parser
        {
            let mut reader = pty_pair
                .master
                .try_clone_reader()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            let parser = parser.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(size) => {
                            if let Ok(mut parser) = parser.write() {
                                parser.process(&buf[..size]);
                            }
                        }
                    }
                }
            });
        }

        // Channel for writing keyboard input to the PTY
        let (tx, mut rx) = channel::<Bytes>(32);

        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let mut writer = BufWriter::new(writer);
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });

        Ok(Self {
            parser,
            sender: tx,
            master_pty: pty_pair.master,
            exited,
            feature_id,
            agent_id,
        })
    }

    fn resize(&self, size: Size) {
        let rows = size.rows.saturating_sub(2);
        let cols = size.cols.saturating_sub(2);
        if let Ok(mut parser) = self.parser.write() {
            parser.screen_mut().set_size(rows, cols);
        }
        let _ = self.master_pty.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::Relaxed)
    }
}

/// Spawn a PTY pane running an agent with the given role and prompt.
fn spawn_pty_agent(
    size: Size,
    role: &RoleSpec,
    project_dir: &Path,
    prompt: &str,
    agent_id: &str,
    feature_id: Option<String>,
) -> io::Result<PtyPane> {
    let (cmd_name, args) = runner::build_agent_command(role, prompt);

    let mut cmd = CommandBuilder::new(&cmd_name);
    for arg in &args {
        cmd.arg(arg);
    }
    cmd.cwd(project_dir);
    cmd.env("FORGE_AGENT_ID", agent_id);

    PtyPane::new(size, cmd, feature_id, agent_id.to_string())
}

/// Open a new pane for the next claimable feature. Returns the feature ID if found.
fn open_next_feature_pane(
    panes: &mut Vec<PtyPane>,
    active_pane: &mut Option<usize>,
    size: Size,
    config: &RunConfig,
) -> Option<String> {
    let features = FeatureList::load(&config.project_dir).ok()?;
    let next = features.next_claimable()?;
    let feature_id = next.id.clone();

    let agent_id = format!("agent-{}", panes.len() + 1);
    let prompt = format!(
        "You are a forge agent. Your assigned feature is {feature_id}. \
         Read features.json for details. Follow the forge-protocol skill. \
         When done, set status to done and exit.",
    );

    match spawn_pty_agent(
        size,
        &config.protocol,
        &config.project_dir,
        &prompt,
        &agent_id,
        Some(feature_id.clone()),
    ) {
        Ok(pane) => {
            let idx = panes.len();
            panes.push(pane);
            *active_pane = Some(idx);
            Some(feature_id)
        }
        Err(_) => None,
    }
}

/// Route keyboard input to a PTY pane. Returns true if the event was handled.
async fn handle_pane_key_event(pane: &mut PtyPane, key: &KeyEvent) -> bool {
    let input_bytes = match key.code {
        KeyCode::Char(ch) => {
            let mut send = vec![ch as u8];
            let upper = ch.to_ascii_uppercase();
            if key.modifiers == KeyModifiers::CONTROL {
                match upper {
                    '2' | '@' | ' ' => send = vec![0],
                    '3' | '[' => send = vec![27],
                    '4' | '\\' => send = vec![28],
                    '5' | ']' => send = vec![29],
                    '6' | '^' => send = vec![30],
                    '7' | '-' | '_' => send = vec![31],
                    c if ('A'..='_').contains(&c) => {
                        send = vec![c as u8 - 64];
                    }
                    _ => {}
                }
            }
            send
        }
        KeyCode::Enter => vec![b'\n'],
        KeyCode::Backspace => vec![8],
        KeyCode::Left => vec![27, 91, 68],
        KeyCode::Right => vec![27, 91, 67],
        KeyCode::Up => vec![27, 91, 65],
        KeyCode::Down => vec![27, 91, 66],
        KeyCode::Tab => vec![9],
        KeyCode::Home => vec![27, 91, 72],
        KeyCode::End => vec![27, 91, 70],
        KeyCode::PageUp => vec![27, 91, 53, 126],
        KeyCode::PageDown => vec![27, 91, 54, 126],
        KeyCode::BackTab => vec![27, 91, 90],
        KeyCode::Delete => vec![27, 91, 51, 126],
        KeyCode::Insert => vec![27, 91, 50, 126],
        KeyCode::Esc => vec![27],
        _ => return true,
    };

    pane.sender.send(Bytes::from(input_bytes)).await.ok();
    true
}

fn cleanup_exited_panes(panes: &mut Vec<PtyPane>, active_pane: &mut Option<usize>) {
    let mut i = 0;
    while i < panes.len() {
        if !panes[i].is_alive() {
            let _removed = panes.remove(i);
            if let Some(active) = active_pane {
                match (*active).cmp(&i) {
                    std::cmp::Ordering::Greater => {
                        *active = active.saturating_sub(1);
                    }
                    std::cmp::Ordering::Equal => {
                        if panes.is_empty() {
                            *active_pane = None;
                        } else if i >= panes.len() {
                            *active_pane = Some(panes.len() - 1);
                        }
                    }
                    std::cmp::Ordering::Less => {}
                }
            }
        } else {
            i += 1;
        }
    }
}

fn calc_pane_size(size: Size, nr_panes: usize) -> Size {
    let nr = std::cmp::max(nr_panes, 1) as u16;
    Size {
        rows: size.rows.saturating_sub(3) / nr,
        cols: size.cols,
    }
}

fn resize_all_panes(panes: &mut [PtyPane], size: Size) {
    for pane in panes.iter() {
        pane.resize(size);
    }
}

fn load_status_counts(project_dir: &Path) -> StatusCounts {
    FeatureList::load(project_dir)
        .map(|f| f.status_counts())
        .unwrap_or_default()
}

fn render_status_bar(counts: &StatusCounts, area: Rect, frame: &mut ratatui::Frame) {
    let pct = if counts.total > 0 {
        (counts.done as f64 / counts.total as f64) * 100.0
    } else {
        0.0
    };

    let status_text = format!(
        " Features: {}/{} done ({pct:.0}%) | {} pending | {} claimed | {} blocked ",
        counts.done, counts.total, counts.pending, counts.claimed, counts.blocked,
    );

    let help_text = " Ctrl+J/K: switch | Ctrl+N: new | Ctrl+X: close | Ctrl+Q: quit ";

    let bar = Line::from(vec![
        Span::styled(
            status_text,
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            help_text,
            Style::default().fg(Color::Gray).bg(Color::DarkGray),
        ),
    ]);

    let paragraph = Paragraph::new(bar).alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

/// Main TUI entry point. Spawns agents in PTY panes and renders them.
pub async fn run_tui(config: &RunConfig) -> io::Result<()> {
    // Set up panic hook to restore terminal
    std::panic::set_hook(Box::new(|panic| {
        ratatui::restore();
        eprintln!("Panic: {panic}");
    }));

    let mut terminal = ratatui::init();

    let mut size = Size {
        rows: terminal.size()?.height,
        cols: terminal.size()?.width,
    };

    let mut panes: Vec<PtyPane> = Vec::new();
    let mut active_pane: Option<usize> = None;
    let mut status_counts = load_status_counts(&config.project_dir);
    let mut status_tick = 0u32;

    // Open first pane with the next claimable feature
    let pane_size = calc_pane_size(size, 1);
    open_next_feature_pane(&mut panes, &mut active_pane, pane_size, config);

    if panes.is_empty() {
        ratatui::restore();
        eprintln!("No claimable features found. Nothing to do.");
        return Ok(());
    }

    let project_dir = config.project_dir.clone();

    loop {
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());

            let pane_area = chunks[0];
            let status_area = chunks[1];

            if panes.is_empty() {
                let msg = Paragraph::new("No active panes. Press Ctrl+N to spawn an agent or Ctrl+Q to quit.")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::Yellow));
                frame.render_widget(msg, pane_area);
            } else {
                let nr_panes = panes.len() as u16;
                let pane_height = pane_area.height / std::cmp::max(nr_panes, 1);

                for (index, pane) in panes.iter().enumerate() {
                    let title = match &pane.feature_id {
                        Some(fid) => format!(" {} — {} ", pane.agent_id, fid),
                        None => format!(" {} ", pane.agent_id),
                    };

                    let is_active = Some(index) == active_pane;
                    let border_style = if is_active {
                        Style::default()
                            .fg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };

                    let block = Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .style(border_style);

                    let mut cursor = Cursor::default();
                    if !is_active {
                        cursor.hide();
                    }

                    let pane_chunk = Rect {
                        x: pane_area.x,
                        y: pane_area.y + (index as u16 * pane_height),
                        width: pane_area.width,
                        height: if index as u16 == nr_panes - 1 {
                            pane_area.height - (index as u16 * pane_height)
                        } else {
                            pane_height
                        },
                    };

                    if let Ok(parser) = pane.parser.read() {
                        let screen = parser.screen();
                        let pseudo_term = PseudoTerminal::new(screen)
                            .block(block)
                            .cursor(cursor);
                        frame.render_widget(pseudo_term, pane_chunk);
                    }
                }
            }

            render_status_bar(&status_counts, status_area, frame);
        })?;

        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    // Ctrl+Q: quit
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break;
                    }
                    // Ctrl+N: spawn new agent pane
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let pane_size = calc_pane_size(size, panes.len() + 1);
                        resize_all_panes(&mut panes, pane_size);
                        open_next_feature_pane(&mut panes, &mut active_pane, pane_size, config);
                    }
                    // Ctrl+X: close active pane
                    KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(idx) = active_pane {
                            panes.remove(idx);
                            if panes.is_empty() {
                                active_pane = None;
                            } else {
                                active_pane = Some(idx % panes.len());
                            }
                            let pane_size = calc_pane_size(size, panes.len());
                            resize_all_panes(&mut panes, pane_size);
                        }
                    }
                    // Ctrl+K: previous pane
                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(idx) = active_pane {
                            active_pane = Some(idx.saturating_sub(1));
                        }
                    }
                    // Ctrl+J: next pane
                    KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(idx) = active_pane {
                            if idx < panes.len().saturating_sub(1) {
                                active_pane = Some(idx + 1);
                            }
                        }
                    }
                    // Forward all other keys to the active pane
                    _ => {
                        if let Some(idx) = active_pane {
                            if idx < panes.len() {
                                handle_pane_key_event(&mut panes[idx], &key).await;
                            }
                        }
                    }
                },
                Event::Resize(cols, rows) => {
                    size.rows = rows;
                    size.cols = cols;
                    let pane_size = calc_pane_size(size, panes.len());
                    resize_all_panes(&mut panes, pane_size);
                }
                _ => {}
            }
        }

        // Periodically refresh status counts (~every 2s at 10ms poll)
        status_tick += 1;
        if status_tick >= 200 {
            status_tick = 0;
            status_counts = load_status_counts(&project_dir);
        }

        cleanup_exited_panes(&mut panes, &mut active_pane);
    }

    ratatui::restore();
    Ok(())
}

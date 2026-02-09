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
        rows: u16,
        cols: u16,
        cmd: CommandBuilder,
        feature_id: Option<String>,
        agent_id: String,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let parser = Arc::new(RwLock::new(vt100::Parser::new(rows, cols, 10000)));
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

        // Read PTY output and feed it to vt100::Parser.
        // Uses spawn_blocking because reader.read() is synchronous and would
        // starve the tokio async worker threads if run via tokio::spawn.
        {
            let mut reader = pty_pair
                .master
                .try_clone_reader()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            let parser = parser.clone();
            spawn_blocking(move || {
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

    /// Resize the PTY and vt100 parser to match the given inner area.
    fn resize_to_inner(&self, inner: Rect) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Ok(mut parser) = self.parser.write() {
            let screen = parser.screen();
            if screen.size() != (inner.height, inner.width) {
                parser.screen_mut().set_size(inner.height, inner.width);
            }
        }
        let _ = self.master_pty.resize(PtySize {
            rows: inner.height,
            cols: inner.width,
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
    rows: u16,
    cols: u16,
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

    PtyPane::new(rows, cols, cmd, feature_id, agent_id.to_string())
}

/// Open a new pane for the next claimable feature.
fn open_next_feature_pane(
    panes: &mut Vec<PtyPane>,
    active_pane: &mut Option<usize>,
    inner_rows: u16,
    inner_cols: u16,
    config: &RunConfig,
) -> Option<String> {
    let mut features = FeatureList::load(&config.project_dir).ok()?;
    let next = features.next_claimable()?;
    let feature_id = next.id.clone();

    let agent_id = format!("agent-{}", panes.len() + 1);

    // Claim the feature so other panes don't pick the same one
    let _ = features.claim(&feature_id, &agent_id);
    let _ = features.save(&config.project_dir);
    let prompt = format!(
        "You are a forge agent. Your assigned feature is {feature_id}. \
         Read features.json for details. Follow the forge-protocol skill. \
         When done, set status to done and exit.",
    );

    match spawn_pty_agent(
        inner_rows,
        inner_cols,
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

/// Route keyboard input to a PTY pane.
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
        KeyCode::Enter => vec![b'\r'],
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

fn load_status_counts(project_dir: &Path) -> StatusCounts {
    FeatureList::load(project_dir)
        .map(|f| f.status_counts())
        .unwrap_or_default()
}

fn render_status_bar(
    counts: &StatusCounts,
    command_mode: bool,
    area: Rect,
    frame: &mut ratatui::Frame,
) {
    let pct = if counts.total > 0 {
        (counts.done as f64 / counts.total as f64) * 100.0
    } else {
        0.0
    };

    let status_text = format!(
        " Features: {}/{} done ({pct:.0}%) | {} pending | {} claimed | {} blocked ",
        counts.done, counts.total, counts.pending, counts.claimed, counts.blocked,
    );

    if command_mode {
        let bar = Line::from(vec![
            Span::styled(
                status_text,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " CMD ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " 1-9:goto  j/k:switch  n:new  x:close  q:quit  esc:cancel ",
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::DarkGray),
            ),
        ]);
        let paragraph = Paragraph::new(bar).alignment(Alignment::Left);
        frame.render_widget(paragraph, area);
    } else {
        let bar = Line::from(vec![
            Span::styled(
                status_text,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " Ctrl+G: command mode ",
                Style::default().fg(Color::Gray).bg(Color::DarkGray),
            ),
        ]);
        let paragraph = Paragraph::new(bar).alignment(Alignment::Left);
        frame.render_widget(paragraph, area);
    }
}

/// Compute grid dimensions for N panes.
/// 1-3 panes: single column (vertical stack)
/// 4+ panes: 2 columns, rows = ceil(n/2)
fn grid_dims(n: usize) -> (usize, usize) {
    if n <= 3 {
        (n, 1) // rows, cols
    } else {
        let cols = 2;
        let rows = (n + cols - 1) / cols;
        (rows, cols)
    }
}

/// Compute the Rect for a given pane index within a grid layout.
fn grid_rect(pane_area: Rect, index: usize, total: usize) -> Rect {
    let (rows, cols) = grid_dims(total);
    let col = index % cols;
    let row = index / cols;
    let cell_w = pane_area.width / cols as u16;
    let cell_h = pane_area.height / rows as u16;

    // Last column gets remaining width, last row gets remaining height
    let x = pane_area.x + col as u16 * cell_w;
    let y = pane_area.y + row as u16 * cell_h;
    let w = if col == cols - 1 {
        pane_area.width - col as u16 * cell_w
    } else {
        cell_w
    };
    let h = if row == rows - 1 {
        pane_area.height - row as u16 * cell_h
    } else {
        cell_h
    };
    Rect::new(x, y, w, h)
}

/// Estimate the inner area for initial PTY size (before first draw).
fn estimate_inner(total_rows: u16, total_cols: u16, nr_panes: u16) -> (u16, u16) {
    let (grid_rows, grid_cols) = grid_dims(nr_panes as usize);
    let pane_rows = total_rows.saturating_sub(1) / std::cmp::max(grid_rows as u16, 1);
    let pane_cols = total_cols / std::cmp::max(grid_cols as u16, 1);
    let inner_rows = pane_rows.saturating_sub(2);
    let inner_cols = pane_cols.saturating_sub(2);
    (std::cmp::max(inner_rows, 1), std::cmp::max(inner_cols, 1))
}

/// Check if a key event is Ctrl+G (BEL, 0x07).
fn is_ctrl_g(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL
}

/// Main TUI entry point. Spawns agents in PTY panes and renders them.
pub async fn run_tui(config: &RunConfig) -> io::Result<()> {
    // Set up panic hook to restore terminal
    std::panic::set_hook(Box::new(|panic| {
        ratatui::restore();
        eprintln!("Panic: {panic}");
    }));

    let mut terminal = ratatui::init();

    let term_size = terminal.size()?;

    let mut panes: Vec<PtyPane> = Vec::new();
    let mut active_pane: Option<usize> = None;
    let mut status_counts = load_status_counts(&config.project_dir);
    let mut status_tick = 0u32;
    let mut command_mode = false;

    // Open first pane with estimated inner size
    let (est_rows, est_cols) = estimate_inner(term_size.height, term_size.width, 1);
    open_next_feature_pane(&mut panes, &mut active_pane, est_rows, est_cols, config);

    if panes.is_empty() {
        ratatui::restore();
        eprintln!("No claimable features found. Nothing to do.");
        return Ok(());
    }

    let project_dir = config.project_dir.clone();

    loop {
        terminal.draw(|frame| {
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());

            let pane_area = outer[0];
            let status_area = outer[1];

            if panes.is_empty() {
                let msg = Paragraph::new(
                    "No active panes. Ctrl+G then n to spawn, or Ctrl+G then q to quit.",
                )
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Yellow));
                frame.render_widget(msg, pane_area);
            } else {
                for (index, pane) in panes.iter().enumerate() {
                    let chunk = grid_rect(pane_area, index, panes.len());

                    let pane_num = index + 1;
                    let title = match &pane.feature_id {
                        Some(fid) => format!(" [{}] {} — {} ", pane_num, pane.agent_id, fid),
                        None => format!(" [{}] {} ", pane_num, pane.agent_id),
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

                    let inner = block.inner(chunk);
                    pane.resize_to_inner(inner);

                    let mut cursor = Cursor::default();
                    if !is_active {
                        cursor.hide();
                    }

                    if let Ok(parser) = pane.parser.read() {
                        let screen = parser.screen();
                        let pseudo_term = PseudoTerminal::new(screen)
                            .block(block)
                            .cursor(cursor);
                        frame.render_widget(pseudo_term, chunk);
                    }
                }
            }

            render_status_bar(&status_counts, command_mode, status_area, frame);
        })?;

        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => {
                    if command_mode {
                        // Command mode: interpret next key as a command, then return to normal
                        command_mode = false;
                        match key.code {
                            // 1-9: jump to pane by number
                            KeyCode::Char(c @ '1'..='9') => {
                                let target = (c as usize) - ('1' as usize);
                                if target < panes.len() {
                                    active_pane = Some(target);
                                }
                            }
                            // j or Down: next pane
                            KeyCode::Char('j') | KeyCode::Down => {
                                if let Some(idx) = active_pane {
                                    if idx < panes.len().saturating_sub(1) {
                                        active_pane = Some(idx + 1);
                                    }
                                }
                            }
                            // k or Up: previous pane
                            KeyCode::Char('k') | KeyCode::Up => {
                                if let Some(idx) = active_pane {
                                    active_pane = Some(idx.saturating_sub(1));
                                }
                            }
                            // n: new pane
                            KeyCode::Char('n') => {
                                let ts = terminal.size()?;
                                let nr = panes.len() as u16 + 1;
                                let (r, c) = estimate_inner(ts.height, ts.width, nr);
                                open_next_feature_pane(
                                    &mut panes,
                                    &mut active_pane,
                                    r,
                                    c,
                                    config,
                                );
                            }
                            // x: close active pane
                            KeyCode::Char('x') => {
                                if let Some(idx) = active_pane {
                                    panes.remove(idx);
                                    if panes.is_empty() {
                                        active_pane = None;
                                    } else {
                                        active_pane = Some(idx % panes.len());
                                    }
                                }
                            }
                            // q: quit
                            KeyCode::Char('q') => {
                                break;
                            }
                            // Esc or anything else: cancel command mode
                            _ => {}
                        }
                    } else if is_ctrl_g(&key) {
                        // Enter command mode
                        command_mode = true;
                    } else {
                        // Normal mode: forward everything to the active pane
                        if let Some(idx) = active_pane {
                            if idx < panes.len() {
                                handle_pane_key_event(&mut panes[idx], &key).await;
                            }
                        }
                    }
                }
                Event::Resize(_, _) => {
                    // Panes resized on next draw() via resize_to_inner
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

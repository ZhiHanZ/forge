use std::io::{self, Read as _};
use std::os::unix::io::{FromRawFd, IntoRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::config::RoleSpec;
use crate::features::{FeatureList, FeatureType, StatusCounts};
use crate::runner::{self, RunConfig};

/// Mark an FD as close-on-exec so it doesn't leak to child processes.
fn set_cloexec(fd: RawFd) {
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

/// Set terminal size on a PTY master FD via ioctl(TIOCSWINSZ).
fn set_terminal_size(fd: RawFd, rows: u16, cols: u16) {
    let winsize = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &winsize);
    }
}

struct PtyPane {
    parser: Arc<RwLock<vt100::Parser>>,
    sender: std::sync::mpsc::Sender<Vec<u8>>,
    master_fd: RawFd,
    child_pid: Option<u32>,
    exited: Arc<AtomicBool>,
    feature_id: Option<String>,
    agent_id: String,
    last_size: (u16, u16),
    feature_priority: Option<u32>,
    feature_type: Option<FeatureType>,
}

impl PtyPane {
    fn new(
        rows: u16,
        cols: u16,
        cmd: &str,
        args: &[String],
        cwd: &Path,
        agent_id: String,
        feature_id: Option<String>,
    ) -> io::Result<Self> {
        // Open PTY pair
        let pty = nix::pty::openpty(None, None)
            .map_err(io::Error::other)?;
        let master_fd = pty.master.into_raw_fd();
        let slave_fd = pty.slave.into_raw_fd();

        // Set initial terminal size
        set_terminal_size(master_fd, rows, cols);

        // Mark master as close-on-exec so it doesn't leak to other children
        set_cloexec(master_fd);

        // Dup master FD for reader and writer threads (each owns its dup)
        let reader_fd = unsafe { libc::dup(master_fd) };
        if reader_fd < 0 {
            unsafe {
                libc::close(master_fd);
                libc::close(slave_fd);
            }
            return Err(io::Error::last_os_error());
        }
        set_cloexec(reader_fd);

        let writer_fd = unsafe { libc::dup(master_fd) };
        if writer_fd < 0 {
            unsafe {
                libc::close(master_fd);
                libc::close(slave_fd);
                libc::close(reader_fd);
            }
            return Err(io::Error::last_os_error());
        }
        set_cloexec(writer_fd);

        // Spawn child process with PTY slave as controlling terminal
        let mut command = std::process::Command::new(cmd);
        command.args(args);
        command.current_dir(cwd);
        command.env("FORGE_AGENT_ID", &agent_id);
        unsafe {
            command.pre_exec(move || {
                // Close parent-only FDs in child
                libc::close(master_fd);
                libc::close(reader_fd);
                libc::close(writer_fd);
                // Set up slave as controlling terminal + stdin/stdout/stderr
                if libc::login_tty(slave_fd) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                unsafe {
                    libc::close(master_fd);
                    libc::close(slave_fd);
                    libc::close(reader_fd);
                    libc::close(writer_fd);
                }
                return Err(e);
            }
        };
        let child_pid = Some(child.id());

        // Close slave in parent (child has its own copy after fork)
        unsafe {
            libc::close(slave_fd);
        }

        let parser = Arc::new(RwLock::new(vt100::Parser::new(rows, cols, 10000)));
        let exited = Arc::new(AtomicBool::new(false));

        // Child exit handler thread
        {
            let exited = exited.clone();
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
                exited.store(true, Ordering::Release);
            });
        }

        // Reader thread: 64KB buffer, feeds vt100 parser
        {
            let parser = parser.clone();
            let exited = exited.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                let mut file = unsafe { std::fs::File::from_raw_fd(reader_fd) };
                loop {
                    match file.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut p) = parser.write() {
                                p.process(&buf[..n]);
                            }
                        }
                    }
                }
                exited.store(true, Ordering::Release);
            });
        }

        // Writer thread: synchronous writes with tcdrain (prevents deadlocks)
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                unsafe {
                    libc::write(
                        writer_fd,
                        bytes.as_ptr() as *const libc::c_void,
                        bytes.len(),
                    );
                    libc::tcdrain(writer_fd);
                }
            }
            unsafe {
                libc::close(writer_fd);
            }
        });

        Ok(Self {
            parser,
            sender: tx,
            master_fd,
            child_pid,
            exited,
            feature_id,
            agent_id,
            last_size: (rows, cols),
            feature_priority: None,
            feature_type: None,
        })
    }

    /// Resize the PTY and vt100 parser when dimensions actually change.
    fn resize_to_inner(&mut self, inner: Rect) {
        let new_size = (inner.height, inner.width);
        if new_size == self.last_size || inner.width == 0 || inner.height == 0 {
            return;
        }
        self.last_size = new_size;
        if let Ok(mut parser) = self.parser.write() {
            parser.screen_mut().set_size(inner.height, inner.width);
        }
        set_terminal_size(self.master_fd, inner.height, inner.width);
    }

    fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }

    fn kill(&self) {
        if let Some(pid) = self.child_pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGHUP);
            }
        }
    }
}

impl Drop for PtyPane {
    fn drop(&mut self) {
        self.kill();
        if self.master_fd >= 0 {
            unsafe {
                libc::close(self.master_fd);
            }
            self.master_fd = -1;
        }
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
    PtyPane::new(
        rows,
        cols,
        &cmd_name,
        &args,
        project_dir,
        agent_id.to_string(),
        feature_id,
    )
}

/// Open a new pane for the next claimable feature.
/// When `completed_id` is provided, prefers features that depend on it (DAG-first).
fn open_next_feature_pane(
    panes: &mut Vec<PtyPane>,
    active_pane: &mut Option<usize>,
    inner_rows: u16,
    inner_cols: u16,
    config: &RunConfig,
    completed_id: Option<&str>,
    next_agent_id: &mut u32,
) -> Option<String> {
    let mut features = FeatureList::load(&config.project_dir).ok()?;
    let next = match completed_id {
        Some(cid) => features.next_after(cid)?,
        None => features.next_claimable()?,
    };
    let feature_id = next.id.clone();
    let priority = next.priority;
    let ftype = next.feature_type.clone();

    *next_agent_id += 1;
    let agent_id = format!("agent-{next_agent_id}");

    // Claim the feature so other panes don't pick the same one
    let _ = features.claim(&feature_id, &agent_id);
    let _ = features.save(&config.project_dir);
    let prompt = runner::build_agent_prompt(&config.project_dir, &feature_id);

    // Use orchestrating role for review features (milestone gates benefit from
    // a different model), protocol role for implement/poc features.
    let role = match ftype {
        FeatureType::Review => &config.orchestrating,
        _ => &config.protocol,
    };

    match spawn_pty_agent(
        inner_rows,
        inner_cols,
        role,
        &config.project_dir,
        &prompt,
        &agent_id,
        Some(feature_id.clone()),
    ) {
        Ok(mut pane) => {
            pane.feature_priority = Some(priority);
            pane.feature_type = Some(ftype);
            let idx = panes.len();
            panes.push(pane);
            *active_pane = Some(idx);
            Some(feature_id)
        }
        Err(_) => None,
    }
}

/// Route keyboard input to a PTY pane.
fn handle_pane_key_event(sender: &std::sync::mpsc::Sender<Vec<u8>>, key: &KeyEvent) -> bool {
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

    let _ = sender.send(input_bytes);
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
    cocoindex_status: &str,
    working_info: &str,
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

    let idx_span = if !cocoindex_status.is_empty() {
        format!(" [idx: {}] ", cocoindex_status)
    } else {
        String::new()
    };

    let working_span = if !working_info.is_empty() {
        format!(" working: {} ", working_info)
    } else {
        String::new()
    };

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
                idx_span,
                Style::default().fg(Color::Cyan).bg(Color::DarkGray),
            ),
            Span::styled(
                working_span,
                Style::default().fg(Color::Green).bg(Color::DarkGray),
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
                idx_span,
                Style::default().fg(Color::Cyan).bg(Color::DarkGray),
            ),
            Span::styled(
                working_span,
                Style::default().fg(Color::Green).bg(Color::DarkGray),
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
    let is_last_alone = index == total - 1 && total % cols != 0;
    let w = if is_last_alone {
        // Last pane is alone in its row — span full width
        pane_area.width
    } else if col == cols - 1 {
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
#[allow(clippy::unused_async)]
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
    let mut next_agent_id: u32 = 0;

    // CocoIndex status tracking (non-blocking)
    #[derive(Clone, Copy, PartialEq)]
    enum CocoStatus { Idle, Running, Done, Unavailable, Error }
    let cocoindex_status = Arc::new(std::sync::Mutex::new(CocoStatus::Idle));

    // Sync CocoIndex context flow files and refresh packages
    crate::context_flow::sync_context_flow(&config.project_dir);
    let _ = crate::context_flow::refresh_context(&config.project_dir);

    // Open first pane with estimated inner size
    let (est_rows, est_cols) = estimate_inner(term_size.height, term_size.width, 1);
    open_next_feature_pane(&mut panes, &mut active_pane, est_rows, est_cols, config, None, &mut next_agent_id);

    if panes.is_empty() {
        ratatui::restore();
        eprintln!("No claimable features found. Nothing to do.");
        return Ok(());
    }

    let project_dir = config.project_dir.clone();

    loop {
        // Build working info string from live panes
        let working_info: String = panes
            .iter()
            .filter_map(|p| {
                let fid = p.feature_id.as_deref()?;
                let pri = p.feature_priority?;
                Some(format!("{}:P{}", fid, pri))
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Read cocoindex status
        let coco_str = {
            let st = cocoindex_status.lock().unwrap();
            match *st {
                CocoStatus::Idle => "",
                CocoStatus::Running => "syncing",
                CocoStatus::Done => "ok",
                CocoStatus::Unavailable => "",
                CocoStatus::Error => "err",
            }
            .to_string()
        };

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
                let num_panes = panes.len();
                for (index, pane) in panes.iter_mut().enumerate() {
                    let chunk = grid_rect(pane_area, index, num_panes);

                    let pane_num = index + 1;
                    let title = match (&pane.feature_id, pane.feature_priority, &pane.feature_type) {
                        (Some(fid), Some(pri), Some(ft)) => {
                            let type_tag = match ft {
                                FeatureType::Implement => "impl",
                                FeatureType::Review => "review",
                                FeatureType::Poc => "poc",
                            };
                            format!(" [{}] {} — {} P{} {} ", pane_num, pane.agent_id, fid, pri, type_tag)
                        }
                        (Some(fid), _, _) => format!(" [{}] {} — {} ", pane_num, pane.agent_id, fid),
                        _ => format!(" [{}] {} ", pane_num, pane.agent_id),
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

            render_status_bar(&status_counts, command_mode, &coco_str, &working_info, status_area, frame);
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
                                    None,
                                    &mut next_agent_id,
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
                                handle_pane_key_event(&panes[idx].sender, &key);
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

        // Replace exited panes with next available features
        let mut i = 0;
        while i < panes.len() {
            if !panes[i].is_alive() {
                let completed_id = panes[i].feature_id.clone();
                panes.remove(i);
                // Non-blocking cocoindex refresh
                {
                    let status = cocoindex_status.clone();
                    let dir = config.project_dir.clone();
                    std::thread::spawn(move || {
                        *status.lock().unwrap() = CocoStatus::Running;
                        match crate::context_flow::refresh_context(&dir) {
                            Ok(true) => *status.lock().unwrap() = CocoStatus::Done,
                            Ok(false) => *status.lock().unwrap() = CocoStatus::Unavailable,
                            Err(_) => *status.lock().unwrap() = CocoStatus::Error,
                        }
                    });
                }
                // Try to spawn a replacement — prefer DAG successors of completed feature
                let ts = terminal.size()?;
                let nr = panes.len() as u16 + 1;
                let (r, c) = estimate_inner(ts.height, ts.width, nr);
                if open_next_feature_pane(
                    &mut panes, &mut active_pane, r, c, config,
                    completed_id.as_deref(),
                    &mut next_agent_id,
                ).is_none() {
                    // No more features — adjust active pane index
                    if panes.is_empty() {
                        active_pane = None;
                    } else if let Some(active) = active_pane {
                        if active >= panes.len() {
                            active_pane = Some(panes.len() - 1);
                        }
                    }
                }
                // Don't increment i — the replacement (or shifted element) is at the same index
            } else {
                i += 1;
            }
        }

        // If all panes are gone and no features left, exit
        if panes.is_empty() {
            status_counts = load_status_counts(&project_dir);
            if status_counts.pending == 0 && status_counts.claimed == 0 {
                break;
            }
        }
    }

    ratatui::restore();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // ── grid_dims tests ──────────────────────────────────────────────

    #[test]
    fn grid_dims_1() {
        assert_eq!(grid_dims(1), (1, 1));
    }

    #[test]
    fn grid_dims_2() {
        assert_eq!(grid_dims(2), (2, 1));
    }

    #[test]
    fn grid_dims_3() {
        assert_eq!(grid_dims(3), (3, 1));
    }

    #[test]
    fn grid_dims_4() {
        assert_eq!(grid_dims(4), (2, 2));
    }

    #[test]
    fn grid_dims_5() {
        assert_eq!(grid_dims(5), (3, 2));
    }

    #[test]
    fn grid_dims_6() {
        assert_eq!(grid_dims(6), (3, 2));
    }

    #[test]
    fn grid_dims_7() {
        assert_eq!(grid_dims(7), (4, 2));
    }

    #[test]
    fn grid_dims_8() {
        assert_eq!(grid_dims(8), (4, 2));
    }

    #[test]
    fn grid_dims_9() {
        assert_eq!(grid_dims(9), (5, 2));
    }

    // ── grid_rect tests ──────────────────────────────────────────────

    /// Helper: check that a set of rects fully covers an area with no gaps
    /// and no overlaps, and all rects are within bounds.
    fn assert_grid_coverage(area: Rect, total: usize) {
        let rects: Vec<Rect> = (0..total).map(|i| grid_rect(area, i, total)).collect();

        // All rects within bounds
        for (i, r) in rects.iter().enumerate() {
            assert!(
                r.x >= area.x
                    && r.y >= area.y
                    && r.x + r.width <= area.x + area.width
                    && r.y + r.height <= area.y + area.height,
                "pane {i} rect {r:?} out of bounds {area:?}",
            );
            assert!(r.width > 0 && r.height > 0, "pane {i} has zero dimension");
        }

        // No overlap: check all pairs
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let a = rects[i];
                let b = rects[j];
                let overlap_x = a.x < b.x + b.width && b.x < a.x + a.width;
                let overlap_y = a.y < b.y + b.height && b.y < a.y + a.height;
                assert!(
                    !(overlap_x && overlap_y),
                    "panes {i} and {j} overlap: {a:?} vs {b:?}",
                );
            }
        }

        // Full coverage: sum of all pixel coverage equals area
        let mut covered = vec![vec![false; area.width as usize]; area.height as usize];
        for r in &rects {
            for dy in 0..r.height {
                for dx in 0..r.width {
                    let py = (r.y - area.y + dy) as usize;
                    let px = (r.x - area.x + dx) as usize;
                    assert!(!covered[py][px], "pixel ({px},{py}) covered twice");
                    covered[py][px] = true;
                }
            }
        }
        for py in 0..area.height as usize {
            for px in 0..area.width as usize {
                assert!(covered[py][px], "pixel ({px},{py}) not covered");
            }
        }
    }

    #[test]
    fn grid_rect_coverage_1_to_9() {
        let area = Rect::new(0, 0, 100, 80);
        for n in 1..=9 {
            assert_grid_coverage(area, n);
        }
    }

    #[test]
    fn grid_rect_single_pane_full_area() {
        let area = Rect::new(0, 0, 100, 80);
        let r = grid_rect(area, 0, 1);
        assert_eq!(r, area);
    }

    #[test]
    fn grid_rect_odd_last_pane_spans_full_width() {
        let area = Rect::new(0, 0, 100, 80);
        for &total in &[5, 7, 9] {
            let last = grid_rect(area, total - 1, total);
            assert_eq!(
                last.width, area.width,
                "for {total} panes, last pane width={} expected={}",
                last.width, area.width,
            );
            assert_eq!(
                last.x, area.x,
                "for {total} panes, last pane x={} expected={}",
                last.x, area.x,
            );
        }
    }

    #[test]
    fn grid_rect_with_offset_area() {
        let area = Rect::new(5, 10, 100, 80);
        for n in 1..=9 {
            assert_grid_coverage(area, n);
        }
    }

    // ── estimate_inner tests ─────────────────────────────────────────

    #[test]
    fn estimate_inner_positive_dimensions() {
        for nr in 1..=9u16 {
            let (rows, cols) = estimate_inner(80, 200, nr);
            assert!(rows > 0, "rows should be positive for {nr} panes");
            assert!(cols > 0, "cols should be positive for {nr} panes");
        }
    }

    #[test]
    fn estimate_inner_smaller_than_outer() {
        let (rows, cols) = estimate_inner(80, 200, 1);
        // inner must be strictly smaller (borders + status bar consume space)
        assert!(rows < 80);
        assert!(cols < 200);
    }

    #[test]
    fn estimate_inner_shrinks_with_more_panes() {
        let (r1, c1) = estimate_inner(80, 200, 1);
        let (r4, c4) = estimate_inner(80, 200, 4);
        // More panes → smaller per-pane area
        assert!(r4 < r1, "rows should shrink: {r4} vs {r1}");
        assert!(c4 < c1, "cols should shrink: {c4} vs {c1}");
    }

    // ── is_ctrl_g tests ──────────────────────────────────────────────

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn is_ctrl_g_true() {
        assert!(is_ctrl_g(&make_key(KeyCode::Char('g'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn is_ctrl_g_plain_g() {
        assert!(!is_ctrl_g(&make_key(KeyCode::Char('g'), KeyModifiers::NONE)));
    }

    #[test]
    fn is_ctrl_g_ctrl_h() {
        assert!(!is_ctrl_g(&make_key(KeyCode::Char('h'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn is_ctrl_g_shift_g() {
        assert!(!is_ctrl_g(&make_key(KeyCode::Char('g'), KeyModifiers::SHIFT)));
    }

    // ── handle_pane_key_event tests ──────────────────────────────────

    fn send_key_and_recv(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let key = make_key(code, modifiers);
        handle_pane_key_event(&tx, &key);
        drop(tx);
        let mut result = Vec::new();
        while let Ok(bytes) = rx.recv() {
            result.extend_from_slice(&bytes);
        }
        result
    }

    #[test]
    fn key_event_enter() {
        let bytes = send_key_and_recv(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(bytes, vec![0x0D]);
    }

    #[test]
    fn key_event_char_a() {
        let bytes = send_key_and_recv(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(bytes, vec![b'a']);
    }

    #[test]
    fn key_event_ctrl_c() {
        let bytes = send_key_and_recv(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(bytes, vec![3]); // 'C' - 64 = 3
    }

    #[test]
    fn key_event_esc() {
        let bytes = send_key_and_recv(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(bytes, vec![27]);
    }

    #[test]
    fn key_event_arrow_up() {
        let bytes = send_key_and_recv(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(bytes, vec![27, 91, 65]);
    }

    #[test]
    fn key_event_arrow_down() {
        let bytes = send_key_and_recv(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(bytes, vec![27, 91, 66]);
    }

    #[test]
    fn key_event_arrow_right() {
        let bytes = send_key_and_recv(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(bytes, vec![27, 91, 67]);
    }

    #[test]
    fn key_event_arrow_left() {
        let bytes = send_key_and_recv(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(bytes, vec![27, 91, 68]);
    }

    #[test]
    fn key_event_tab() {
        let bytes = send_key_and_recv(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(bytes, vec![9]);
    }

    #[test]
    fn key_event_backspace() {
        let bytes = send_key_and_recv(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(bytes, vec![8]);
    }

    #[test]
    fn key_event_unhandled_sends_nothing() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let key = make_key(KeyCode::F(1), KeyModifiers::NONE);
        handle_pane_key_event(&tx, &key);
        drop(tx);
        assert!(rx.recv().is_err());
    }

    // ── cleanup_exited_panes tests ───────────────────────────────────

    fn mock_pane(agent_id: &str, exited: bool) -> PtyPane {
        let (tx, _rx) = std::sync::mpsc::channel::<Vec<u8>>();
        PtyPane {
            parser: Arc::new(RwLock::new(vt100::Parser::new(24, 80, 0))),
            sender: tx,
            master_fd: -1,
            child_pid: None,
            exited: Arc::new(AtomicBool::new(exited)),
            feature_id: None,
            agent_id: agent_id.to_string(),
            last_size: (24, 80),
            feature_priority: None,
            feature_type: None,
        }
    }

    #[test]
    fn cleanup_all_alive() {
        let mut panes = vec![
            mock_pane("a1", false),
            mock_pane("a2", false),
            mock_pane("a3", false),
        ];
        let mut active = Some(1usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 3);
        assert_eq!(active, Some(1));
    }

    #[test]
    fn cleanup_first_dead_active_zero() {
        let mut panes = vec![
            mock_pane("a1", true),
            mock_pane("a2", false),
            mock_pane("a3", false),
        ];
        let mut active = Some(0usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 2);
        // active was pointing to the removed pane (index 0), should stay at 0
        // since there are still panes
        assert_eq!(active, Some(0));
    }

    #[test]
    fn cleanup_middle_dead_active_above() {
        let mut panes = vec![
            mock_pane("a1", false),
            mock_pane("a2", true),
            mock_pane("a3", false),
        ];
        let mut active = Some(2usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 2);
        assert_eq!(active, Some(1)); // decremented because removed index was below
    }

    #[test]
    fn cleanup_last_dead() {
        let mut panes = vec![
            mock_pane("a1", false),
            mock_pane("a2", false),
            mock_pane("a3", true),
        ];
        let mut active = Some(2usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 2);
        assert_eq!(active, Some(1)); // clamped to last index
    }

    #[test]
    fn cleanup_all_dead() {
        let mut panes = vec![
            mock_pane("a1", true),
            mock_pane("a2", true),
            mock_pane("a3", true),
        ];
        let mut active = Some(1usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 0);
        assert_eq!(active, None);
    }

    #[test]
    fn cleanup_multiple_dead_scattered() {
        let mut panes = vec![
            mock_pane("a1", true),
            mock_pane("a2", false),
            mock_pane("a3", true),
            mock_pane("a4", false),
        ];
        let mut active = Some(3usize);
        cleanup_exited_panes(&mut panes, &mut active);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].agent_id, "a2");
        assert_eq!(panes[1].agent_id, "a4");
        // active was 3 -> two panes removed before/at it, should be 1
        assert_eq!(active, Some(1));
    }

    // ── render_status_bar tests ──────────────────────────────────────

    fn render_status_bar_to_string(counts: &StatusCounts, command_mode: bool) -> String {
        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_status_bar(counts, command_mode, "", "", area, frame);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        // Collect all non-empty text from the buffer
        let mut text = String::new();
        for x in 0..buf.area.width {
            let cell = &buf[(x, 0)];
            text.push_str(cell.symbol());
        }
        text
    }

    #[test]
    fn status_bar_normal_mode() {
        let counts = StatusCounts {
            total: 10,
            pending: 3,
            claimed: 2,
            done: 4,
            blocked: 1,
        };
        let text = render_status_bar_to_string(&counts, false);
        assert!(text.contains("4/10 done (40%)"), "got: {text}");
        assert!(text.contains("3 pending"), "got: {text}");
        assert!(text.contains("Ctrl+G: command mode"), "got: {text}");
    }

    #[test]
    fn status_bar_command_mode() {
        let counts = StatusCounts {
            total: 5,
            pending: 1,
            claimed: 1,
            done: 2,
            blocked: 1,
        };
        let text = render_status_bar_to_string(&counts, true);
        assert!(text.contains("CMD"), "got: {text}");
        assert!(text.contains("q:quit"), "got: {text}");
        assert!(text.contains("n:new"), "got: {text}");
    }

    #[test]
    fn status_bar_zero_features() {
        let counts = StatusCounts {
            total: 0,
            pending: 0,
            claimed: 0,
            done: 0,
            blocked: 0,
        };
        let text = render_status_bar_to_string(&counts, false);
        assert!(text.contains("0/0 done (0%)"), "got: {text}");
    }

    // ── resize debounce tests ────────────────────────────────────────

    #[test]
    fn resize_debounce_skips_same_size() {
        let mut pane = mock_pane("a1", false);
        pane.last_size = (24, 80);
        // Same size should be a no-op
        pane.resize_to_inner(Rect::new(0, 0, 80, 24));
        assert_eq!(pane.last_size, (24, 80));
    }

    #[test]
    fn resize_debounce_updates_on_change() {
        let mut pane = mock_pane("a1", false);
        pane.last_size = (24, 80);
        // Different size should update
        pane.resize_to_inner(Rect::new(0, 0, 120, 40));
        assert_eq!(pane.last_size, (40, 120));
    }

    #[test]
    fn resize_debounce_skips_zero_dimensions() {
        let mut pane = mock_pane("a1", false);
        pane.last_size = (24, 80);
        pane.resize_to_inner(Rect::new(0, 0, 0, 24));
        assert_eq!(pane.last_size, (24, 80));
        pane.resize_to_inner(Rect::new(0, 0, 80, 0));
        assert_eq!(pane.last_size, (24, 80));
    }

    // ── kill / signal tests ──────────────────────────────────────────

    #[test]
    fn kill_sends_sighup() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut pane = mock_pane("a1", false);
        pane.child_pid = Some(pid);
        pane.kill();
        std::thread::sleep(Duration::from_millis(100));
        // SIGHUP should have terminated the process
        let status = child.try_wait().unwrap();
        assert!(status.is_some(), "process should have exited after SIGHUP");
        // Prevent Drop from sending another SIGHUP (harmless but clean)
        pane.child_pid = None;
    }

    // ── ANSI fixture tests (zellij pattern: feed bytes, check screen) ──

    /// Feed raw bytes into a vt100::Parser and return screen contents.
    /// This is the zellij pattern: no PTY needed, tests parser rendering.
    fn feed_bytes(rows: u16, cols: u16, bytes: &[u8]) -> String {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(bytes);
        parser.screen().contents()
    }

    #[test]
    fn ansi_plain_text() {
        let screen = feed_bytes(24, 80, b"Hello, world!");
        assert!(screen.contains("Hello, world!"), "got: {screen}");
    }

    #[test]
    fn ansi_newline_and_cr() {
        let screen = feed_bytes(24, 80, b"line1\r\nline2\r\nline3");
        assert!(screen.contains("line1"), "got: {screen}");
        assert!(screen.contains("line2"), "got: {screen}");
        assert!(screen.contains("line3"), "got: {screen}");
    }

    #[test]
    fn ansi_cursor_movement() {
        // Write "AB", move cursor left 1, overwrite with "X" → "AX"
        let screen = feed_bytes(24, 80, b"AB\x1b[1DX");
        assert!(screen.contains("AX"), "got: {screen}");
    }

    #[test]
    fn ansi_erase_line() {
        // Write "Hello", then erase from cursor to end of line
        // \x1b[H moves to home, write "Hello", \r goes to col 0, \x1b[K erases line
        let screen = feed_bytes(24, 80, b"Hello\r\x1b[K");
        // Line should be blank after erase
        assert!(!screen.contains("Hello"), "erase failed, got: {screen}");
    }

    #[test]
    fn ansi_sgr_colors_dont_corrupt_text() {
        // SGR color codes should not appear as text
        // \x1b[31m = red, \x1b[0m = reset
        let screen = feed_bytes(24, 80, b"\x1b[31mRed text\x1b[0m Normal");
        assert!(screen.contains("Red text"), "got: {screen}");
        assert!(screen.contains("Normal"), "got: {screen}");
        assert!(!screen.contains("[31m"), "raw escape leaked: {screen}");
    }

    #[test]
    fn ansi_cursor_save_restore() {
        // DECSC (\x1b7) saves cursor at col 5, DECRC (\x1b8) restores it
        let screen = feed_bytes(24, 80, b"Hello\x1b7 World\x1b8XYZ");
        // After restore, cursor is back at col 5, "XYZ" overwrites " Wo"
        // Result: "HelloXYZrld"
        assert!(screen.contains("HelloXYZrld"), "got: {screen}");
    }

    #[test]
    fn ansi_clear_screen() {
        let screen = feed_bytes(24, 80, b"garbage\x1b[2J\x1b[HClean");
        // \x1b[2J clears screen, \x1b[H homes cursor
        assert!(screen.contains("Clean"), "got: {screen}");
        assert!(!screen.contains("garbage"), "clear failed, got: {screen}");
    }

    #[test]
    fn ansi_line_wrap() {
        // Write exactly 80 chars + 1 more — should wrap to next line
        let line = "A".repeat(80);
        let mut input = line.as_bytes().to_vec();
        input.push(b'B');
        let screen = feed_bytes(24, 80, &input);
        // Both the full line and the wrapped char should be present
        assert!(screen.contains(&"A".repeat(80)), "got: {screen}");
        assert!(screen.contains("B"), "wrap char missing: {screen}");
    }

    #[test]
    fn ansi_alternate_screen() {
        // Switch to alternate screen, write, switch back — original content restored
        let screen = feed_bytes(
            24,
            80,
            b"Main\x1b[?1049hAlternate\x1b[?1049l",
        );
        assert!(screen.contains("Main"), "original lost: {screen}");
        assert!(!screen.contains("Alternate"), "alt leaked: {screen}");
    }

    #[test]
    fn ansi_scroll_region() {
        // Set scroll region to lines 2-4, then scroll within it
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b[1;1HLine1");
        input.extend_from_slice(b"\x1b[2;1HLine2");
        input.extend_from_slice(b"\x1b[3;1HLine3");
        input.extend_from_slice(b"\x1b[4;1HLine4");
        input.extend_from_slice(b"\x1b[5;1HLine5");
        // Set scroll region to rows 2-4
        input.extend_from_slice(b"\x1b[2;4r");
        // Move to row 4 and issue newline (scrolls region)
        input.extend_from_slice(b"\x1b[4;1H\n");
        input.extend_from_slice(b"New");
        let screen = feed_bytes(10, 80, &input);
        // Line1 should be untouched (above region)
        assert!(screen.contains("Line1"), "got: {screen}");
        // Line5 should be untouched (below region)
        assert!(screen.contains("Line5"), "got: {screen}");
    }

    #[test]
    fn ansi_large_burst() {
        // Simulate a large output burst (like `ls -la` on a big directory)
        let mut input = Vec::new();
        for i in 0..500 {
            input.extend_from_slice(format!("file_{i:04}.txt\r\n").as_bytes());
        }
        let screen = feed_bytes(24, 80, &input);
        // Last lines should be visible (scrolled up)
        assert!(screen.contains("file_0499.txt"), "got tail: {screen}");
    }

    // ── resize-parser sync tests ─────────────────────────────────────

    #[test]
    fn resize_updates_parser_screen_size() {
        let mut pane = mock_pane("a1", false);
        // Initial size from mock_pane is (24, 80)
        assert_eq!(pane.parser.read().unwrap().screen().size(), (24, 80));
        // Resize to something different
        pane.resize_to_inner(Rect::new(0, 0, 120, 40));
        assert_eq!(pane.parser.read().unwrap().screen().size(), (40, 120));
    }

    #[test]
    fn resize_preserves_existing_content() {
        let mut pane = mock_pane("a1", false);
        // Feed some content
        pane.parser.write().unwrap().process(b"Hello from pane");
        // Resize
        pane.resize_to_inner(Rect::new(0, 0, 60, 20));
        // Content should survive
        let screen = pane.parser.read().unwrap().screen().contents();
        assert!(screen.contains("Hello from pane"), "got: {screen}");
    }

    #[test]
    fn resize_then_feed_uses_new_dimensions() {
        let mut pane = mock_pane("a1", false);
        // Resize to narrow terminal
        pane.resize_to_inner(Rect::new(0, 0, 20, 10));
        // Feed a long line — should wrap at col 20
        let long = "X".repeat(25);
        pane.parser.write().unwrap().process(long.as_bytes());
        let screen = pane.parser.read().unwrap().screen().contents();
        // All 25 chars should be present (wrapped across lines)
        let x_count = screen.chars().filter(|&c| c == 'X').count();
        assert_eq!(x_count, 25, "got {x_count} X's in: {screen}");
    }

    // ── integration: real PTY tests ──────────────────────────────────

    #[test]
    #[ignore] // requires real PTY — run with: cargo test -- --ignored
    fn pty_spawn_echo_roundtrip() {
        let pane = PtyPane::new(
            24,
            80,
            "echo",
            &["hello".into()],
            Path::new("/tmp"),
            "test-1".into(),
            None,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(!pane.is_alive());
        let parser = pane.parser.read().unwrap();
        let contents = parser.screen().contents();
        assert!(contents.contains("hello"), "got: {contents}");
    }

    #[test]
    #[ignore]
    fn pty_writer_under_pressure() {
        // Spawn `cat` which reads stdin and echoes to stdout simultaneously.
        // This is the scenario that deadlocks with async writers (vim, etc).
        // Send a burst of data and verify it all arrives without hanging.
        let pane = PtyPane::new(
            24,
            80,
            "cat",
            &[],
            Path::new("/tmp"),
            "pressure-1".into(),
            None,
        )
        .unwrap();

        // Send 1000 lines through the writer thread
        for i in 0..1000 {
            let line = format!("line {i}\r");
            let _ = pane.sender.send(line.into_bytes());
        }

        // Wait for output to arrive (with timeout — deadlock = test hangs)
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_last = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            if let Ok(parser) = pane.parser.read() {
                let contents = parser.screen().contents();
                if contents.contains("line 999") {
                    saw_last = true;
                    break;
                }
            }
        }
        assert!(saw_last, "timed out — possible deadlock");
    }

    #[test]
    #[ignore]
    fn pty_resize_updates_child_stty() {
        // Spawn bash, resize the PTY, then ask `stty size` to confirm
        // the child process sees the new dimensions.
        let pane = PtyPane::new(
            24,
            80,
            "bash",
            &["--norc".into(), "--noprofile".into()],
            Path::new("/tmp"),
            "resize-1".into(),
            None,
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(300));

        // Resize to 40x120
        set_terminal_size(pane.master_fd, 40, 120);

        // Small delay for terminal to process the SIGWINCH
        std::thread::sleep(Duration::from_millis(100));

        // Ask stty for the size
        let _ = pane.sender.send(b"stty size\r".to_vec());
        std::thread::sleep(Duration::from_millis(500));

        let contents = pane.parser.read().unwrap().screen().contents();
        assert!(
            contents.contains("40 120"),
            "child didn't see resize, got: {contents}",
        );
    }
}

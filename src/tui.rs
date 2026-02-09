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
async fn handle_pane_key_event(sender: &Sender<Bytes>, key: &KeyEvent) -> bool {
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

    sender.send(Bytes::from(input_bytes)).await.ok();
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
                                handle_pane_key_event(&panes[idx].sender, &key).await;
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
                panes.remove(i);
                // Try to spawn a replacement with the next claimable feature
                let ts = terminal.size()?;
                let nr = panes.len() as u16 + 1;
                let (r, c) = estimate_inner(ts.height, ts.width, nr);
                if open_next_feature_pane(&mut panes, &mut active_pane, r, c, config).is_none() {
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

    async fn send_key_and_recv(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(32);
        let key = make_key(code, modifiers);
        handle_pane_key_event(&tx, &key).await;
        drop(tx);
        let mut result = Vec::new();
        while let Some(bytes) = rx.recv().await {
            result.extend_from_slice(&bytes);
        }
        result
    }

    #[tokio::test]
    async fn key_event_enter() {
        let bytes = send_key_and_recv(KeyCode::Enter, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![0x0D]);
    }

    #[tokio::test]
    async fn key_event_char_a() {
        let bytes = send_key_and_recv(KeyCode::Char('a'), KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![b'a']);
    }

    #[tokio::test]
    async fn key_event_ctrl_c() {
        let bytes = send_key_and_recv(KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert_eq!(bytes, vec![3]); // 'C' - 64 = 3
    }

    #[tokio::test]
    async fn key_event_esc() {
        let bytes = send_key_and_recv(KeyCode::Esc, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![27]);
    }

    #[tokio::test]
    async fn key_event_arrow_up() {
        let bytes = send_key_and_recv(KeyCode::Up, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![27, 91, 65]);
    }

    #[tokio::test]
    async fn key_event_arrow_down() {
        let bytes = send_key_and_recv(KeyCode::Down, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![27, 91, 66]);
    }

    #[tokio::test]
    async fn key_event_arrow_right() {
        let bytes = send_key_and_recv(KeyCode::Right, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![27, 91, 67]);
    }

    #[tokio::test]
    async fn key_event_arrow_left() {
        let bytes = send_key_and_recv(KeyCode::Left, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![27, 91, 68]);
    }

    #[tokio::test]
    async fn key_event_tab() {
        let bytes = send_key_and_recv(KeyCode::Tab, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![9]);
    }

    #[tokio::test]
    async fn key_event_backspace() {
        let bytes = send_key_and_recv(KeyCode::Backspace, KeyModifiers::NONE).await;
        assert_eq!(bytes, vec![8]);
    }

    #[tokio::test]
    async fn key_event_unhandled_sends_nothing() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(32);
        let key = make_key(KeyCode::F(1), KeyModifiers::NONE);
        handle_pane_key_event(&tx, &key).await;
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    // ── cleanup_exited_panes tests ───────────────────────────────────

    fn mock_pane(agent_id: &str, exited: bool) -> PtyPane {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty failed in test");
        // Drop the slave immediately — we don't need to spawn anything
        drop(pty_pair.slave);

        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(1);
        PtyPane {
            parser: Arc::new(RwLock::new(vt100::Parser::new(24, 80, 0))),
            sender: tx,
            master_pty: pty_pair.master,
            exited: Arc::new(AtomicBool::new(exited)),
            feature_id: None,
            agent_id: agent_id.to_string(),
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
                render_status_bar(counts, command_mode, area, frame);
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
}

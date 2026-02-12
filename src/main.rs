mod config;
mod context;
mod context_flow;
mod export;
mod features;
mod git;
mod init;
mod runner;
mod skills;
mod template;
mod tui;
mod tui_orchestrator;
mod verify;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "forge", about = "Orchestrate autonomous coding agents")]
struct Cli {
    /// Project directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    project: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

/// All commands are pure orchestration — no LLM calls.
/// Planning is a skill (/forge-planning), not a CLI command.
#[derive(Subcommand)]
enum Commands {
    /// Initialize a forge project: dirs, forge.toml, skills, CLAUDE.md
    Init {
        /// Project description
        description: String,
    },
    /// Start the autonomous development loop
    Run {
        /// Number of parallel agents
        #[arg(long, default_value_t = 1)]
        agents: usize,
        /// Max sessions before stopping
        #[arg(long, default_value_t = 50)]
        max_sessions: usize,
        /// Show TUI dashboard
        #[arg(long)]
        watch: bool,
        /// Override backend for all roles (e.g. claude, codex)
        #[arg(long)]
        backend: Option<String>,
        /// Override model for all roles (e.g. sonnet, o3)
        #[arg(long)]
        model: Option<String>,
    },
    /// Run all verify scripts
    Verify,
    /// Show project status: features, context, progress
    Status,
    /// Install/update project dependencies (skills, CLAUDE.md, permissions)
    Install,
    /// Stop all running agents gracefully
    Stop,
    /// Show agent logs
    Logs {
        /// Agent ID (default: agent-1)
        #[arg(default_value = "agent-1")]
        agent: String,
        /// Number of lines to show from the end
        #[arg(short, long, default_value_t = 50)]
        tail: usize,
    },
    /// Export project data for analysis
    Export {
        /// Output directory (default: .forge/export/)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Skip Claude Code JSONL transcripts
        #[arg(long)]
        no_transcripts: bool,
        /// Git commits to include (default: 100)
        #[arg(long, default_value_t = 100)]
        git_commits: usize,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { description } => cmd_init(&cli.project, &description),
        Commands::Install => cmd_install(&cli.project),
        Commands::Run {
            agents,
            max_sessions,
            watch,
            backend,
            model,
        } => cmd_run(&cli.project, agents, max_sessions, watch, backend, model),
        Commands::Verify => cmd_verify(&cli.project),
        Commands::Status => cmd_status(&cli.project),
        Commands::Stop => cmd_stop(&cli.project),
        Commands::Logs { agent, tail } => cmd_logs(&cli.project, &agent, tail),
        Commands::Export {
            output,
            no_transcripts,
            git_commits,
        } => cmd_export(&cli.project, output, no_transcripts, git_commits),
    }
}

fn cmd_init(project_dir: &PathBuf, description: &str) {
    match init::init_project(project_dir, description) {
        Ok(()) => {
            println!("Initialized forge project in {}", project_dir.display());
            println!();
            println!("Created:");
            println!("  forge.toml              project config");
            println!("  features.json           task list (empty — use /forge-planning to fill)");
            println!("  CLAUDE.md               agent instructions");
            println!("  AGENTS.md               agent instructions (non-Claude)");
            println!("  context/                decisions, gotchas, patterns, references");
            println!("  feedback/               test summaries");
            println!("  scripts/verify/         verify scripts");
            println!("  .claude/skills/         4 skills installed");
            println!("  .agents/skills/         4 skills installed (Codex)");
            println!();
            println!("Next steps:");
            println!("  1. Write DESIGN.md with your project design");
            println!("  2. Run /forge-planning in Claude Code to generate features");
            println!("  3. Run `forge run` to start the development loop");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_install(project_dir: &PathBuf) {
    match init::install_project(project_dir) {
        Ok(()) => {
            println!("Installed forge project in {}", project_dir.display());
            println!();
            println!("Updated:");
            println!("  .claude/skills/         skills reinstalled from binary");
            println!("  .agents/skills/         skills reinstalled from binary (Codex)");
            println!("  CLAUDE.md               regenerated from forge.toml");
            println!("  AGENTS.md               regenerated from forge.toml");
            println!("  context/                directories ensured");
            println!("  scripts/verify/         scripts marked executable");
            println!();

            // Check backend CLIs
            let config = config::ForgeConfig::load(project_dir).unwrap_or_else(|_| {
                config::ForgeConfig::scaffold("unknown", "")
            });

            let mut backends = std::collections::BTreeSet::new();
            backends.insert(config.forge.roles.protocol.backend.as_str());
            backends.insert(config.forge.roles.orchestrating.backend.as_str());
            backends.insert(config.forge.roles.planning.backend.as_str());
            backends.insert(config.forge.roles.adjusting.backend.as_str());

            let mut missing = Vec::new();
            for backend in &backends {
                if std::process::Command::new(backend)
                    .arg("--version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_err()
                {
                    missing.push(*backend);
                }
            }

            if missing.is_empty() {
                println!("Backends: all OK ({})", backends.into_iter().collect::<Vec<_>>().join(", "));
            } else {
                for name in &missing {
                    eprintln!("Warning: backend '{}' not found in PATH", name);
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_run(
    project_dir: &PathBuf,
    agents: usize,
    max_sessions: usize,
    watch: bool,
    backend: Option<String>,
    model: Option<String>,
) {
    // Sync skills to both .claude/skills/ and .agents/skills/ so existing
    // projects work with Codex without requiring re-init.
    if let Err(e) = skills::sync_skills(project_dir) {
        eprintln!("Warning: failed to sync skills: {e}");
    }

    // Load forge config to get role settings
    let forge_config = config::ForgeConfig::load(project_dir).unwrap_or_else(|_| {
        config::ForgeConfig::scaffold("unknown", "")
    });

    let mut protocol = forge_config.forge.roles.protocol.clone();
    let mut orchestrating = forge_config.forge.roles.orchestrating.clone();

    // Apply CLI overrides
    if let Some(ref b) = backend {
        protocol.backend = b.clone();
        orchestrating.backend = b.clone();
    }
    if let Some(ref m) = model {
        protocol.model = m.clone();
        orchestrating.model = m.clone();
    }

    let run_config = runner::RunConfig {
        project_dir: project_dir.clone(),
        protocol,
        orchestrating,
        max_sessions,
        num_agents: agents,
    };

    if watch {
        // TUI mode: spawn agents in interactive PTY panes
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            if let Err(e) = tui::run_tui(&run_config).await {
                eprintln!("TUI error: {e}");
                std::process::exit(1);
            }
        });
        return;
    }

    // Headless mode (original behavior)
    println!(
        "forge run: {} agent(s), backend={}, model={}, max_sessions={}",
        agents, run_config.protocol.backend, run_config.protocol.model, max_sessions
    );
    println!();

    let outcome = if agents > 1 {
        runner::run_multi_agent(&run_config)
    } else {
        runner::run_single_agent(&run_config)
    };

    match outcome {
        runner::RunOutcome::AllDone { sessions } => {
            println!();
            println!("All features done in {sessions} session(s).");
        }
        runner::RunOutcome::MaxSessions {
            sessions,
            remaining,
        } => {
            println!();
            println!("Stopped after {sessions} session(s). {remaining} feature(s) remaining.");
        }
        runner::RunOutcome::Stopped { sessions } => {
            println!();
            println!("Stopped by request after {sessions} session(s).");
        }
        runner::RunOutcome::SpawnError(e) => {
            eprintln!();
            eprintln!("Agent spawn failed: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_verify(project_dir: &PathBuf) {
    match verify::verify_all(project_dir) {
        Ok(results) => {
            if results.is_empty() {
                println!("No features to verify (none are done or claimed).");
                return;
            }

            let mut pass = 0;
            let mut fail = 0;

            for result in &results {
                let status = if result.passed {
                    pass += 1;
                    "PASS"
                } else {
                    fail += 1;
                    "FAIL"
                };
                println!("[{status}] {}", result.feature_id);
                if !result.passed && !result.output.is_empty() {
                    // Show first 5 lines of failure output
                    for line in result.output.lines().take(5) {
                        println!("  {line}");
                    }
                }
            }

            println!();
            println!("{pass} passed, {fail} failed, {} total", results.len());

            if fail > 0 {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_stop(project_dir: &PathBuf) {
    match runner::request_stop(project_dir) {
        Ok(()) => println!("Stop requested. Agents will stop after the current session."),
        Err(e) => {
            eprintln!("Error requesting stop: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_logs(project_dir: &PathBuf, agent: &str, tail: usize) {
    let log_path = project_dir.join(".forge/logs").join(format!("{agent}.log"));
    if !log_path.exists() {
        eprintln!("No log file found for agent '{agent}'");
        eprintln!("  Expected: {}", log_path.display());
        std::process::exit(1);
    }

    match std::fs::read_to_string(&log_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(tail);
            for line in &lines[start..] {
                println!("{line}");
            }
        }
        Err(e) => {
            eprintln!("Error reading log: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_export(
    project_dir: &PathBuf,
    output: Option<PathBuf>,
    no_transcripts: bool,
    git_commits: usize,
) {
    let output_dir = output.unwrap_or_else(|| project_dir.join(".forge/export"));
    let include_transcripts = !no_transcripts;

    match export::export_project(project_dir, &output_dir, include_transcripts, git_commits) {
        Ok(manifest) => {
            println!("Exported to {}", output_dir.display());
            println!();
            println!("Sections: {}", manifest.sections.join(", "));
            println!(
                "Features: {} total ({} done, {} pending)",
                manifest.features.total, manifest.features.done, manifest.features.pending
            );
            if let Some(git) = &manifest.git {
                println!(
                    "Git: {} commits, branch {}, latest {}",
                    git.commits_included, git.branch, git.latest_commit
                );
            }
            if !manifest.transcripts.is_empty() {
                let total_bytes: u64 =
                    manifest.transcripts.iter().map(|t| t.size_bytes).sum();
                println!(
                    "Transcripts: {} sessions ({:.1} MB)",
                    manifest.transcripts.len(),
                    total_bytes as f64 / 1_048_576.0
                );
            }
            println!();
            println!("Manifest: {}", output_dir.join("manifest.json").display());
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_status(project_dir: &PathBuf) {
    // Load features
    let features = match features::FeatureList::load(project_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error loading features: {e}");
            std::process::exit(1);
        }
    };

    let dag = render_feature_dag(&features);
    print!("{dag}");

    // Load context
    let ctx = context::ContextManager::new(project_dir);
    match ctx.counts() {
        Ok(ctx_counts) => {
            let total: usize = ctx_counts.values().sum();
            if total > 0 {
                println!();
                println!("Context: {total} entries");
                let parts: Vec<String> = ctx_counts
                    .iter()
                    .filter(|(_, count)| **count > 0)
                    .map(|(cat, count)| format!("{cat}: {count}"))
                    .collect();
                if !parts.is_empty() {
                    println!("  {}", parts.join(", "));
                }
            }
        }
        Err(_) => {}
    }
}

fn render_feature_dag(features: &features::FeatureList) -> String {
    use features::{FeatureStatus, FeatureType};

    let counts = features.status_counts();
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "Features: {} total ({} done, {} claimed, {} pending",
        counts.total, counts.done, counts.claimed, counts.pending
    ));
    if counts.blocked > 0 {
        out.push_str(&format!(", {} blocked", counts.blocked));
    }
    out.push_str(")\n");

    if counts.total > 0 {
        let pct = (counts.done as f64 / counts.total as f64) * 100.0;
        out.push_str(&format!("Progress: {pct:.0}%\n"));
    }

    if features.features.is_empty() {
        return out;
    }

    let claimable_ids = features.claimable_ids();
    let milestone_groups = features.milestone_claimable();

    // Build a set of "next up" IDs: top 3 from milestone ordering
    let mut next_up_ids: Vec<&str> = Vec::new();
    for (_, ids) in &milestone_groups {
        for id in ids {
            if next_up_ids.len() >= 3 {
                break;
            }
            next_up_ids.push(id);
        }
        if next_up_ids.len() >= 3 {
            break;
        }
    }
    let next_up_set: std::collections::HashSet<&str> =
        next_up_ids.iter().copied().collect();

    // Partition features into display groups
    let mut done: Vec<&features::Feature> = Vec::new();
    let mut claimed: Vec<&features::Feature> = Vec::new();
    let mut claimable: Vec<&features::Feature> = Vec::new();
    let mut pending_blocked_deps: Vec<&features::Feature> = Vec::new();
    let mut blocked: Vec<&features::Feature> = Vec::new();

    for f in &features.features {
        match f.status {
            FeatureStatus::Done => done.push(f),
            FeatureStatus::Claimed => claimed.push(f),
            FeatureStatus::Blocked => blocked.push(f),
            FeatureStatus::Pending => {
                if claimable_ids.contains(&f.id.as_str()) {
                    claimable.push(f);
                } else {
                    pending_blocked_deps.push(f);
                }
            }
        }
    }

    // Sort claimable by milestone order: features in earlier milestones first
    let claimable_set: std::collections::HashSet<&str> =
        claimable_ids.iter().copied().collect();
    let mut milestone_order: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    let mut order = 0;
    for (_, ids) in &milestone_groups {
        for id in ids {
            if claimable_set.contains(id) {
                milestone_order.insert(id, order);
                order += 1;
            }
        }
    }
    claimable.sort_by_key(|f| {
        milestone_order
            .get(f.id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });

    done.sort_by_key(|f| f.priority);
    claimed.sort_by_key(|f| f.priority);
    pending_blocked_deps.sort_by_key(|f| f.priority);
    blocked.sort_by_key(|f| f.priority);

    out.push('\n');

    let type_tag = |t: &FeatureType| -> &str {
        match t {
            FeatureType::Implement => "impl",
            FeatureType::Review => "review",
            FeatureType::Poc => "poc",
        }
    };

    let truncate = |s: &str, max: usize| -> String {
        if s.len() <= max {
            s.to_string()
        } else {
            format!("{}...", &s[..max - 3])
        }
    };

    // Render each group
    for f in &done {
        out.push_str(&format!(
            "  \u{2713} {} [{}]  {}\n",
            f.id,
            type_tag(&f.feature_type),
            truncate(&f.description, 50)
        ));
    }

    for f in &claimed {
        let agent = f.claimed_by.as_deref().unwrap_or("?");
        out.push_str(&format!(
            "  \u{29D7} {} [{}]  {}  ({})\n",
            f.id,
            type_tag(&f.feature_type),
            truncate(&f.description, 50),
            agent
        ));
        if !f.depends_on.is_empty() {
            out.push_str(&format!("    \u{2190} {}\n", f.depends_on.join(", ")));
        }
    }

    for f in &claimable {
        let indicator = if next_up_set.contains(f.id.as_str()) {
            "\u{25B8}"
        } else {
            "\u{00B7}"
        };
        out.push_str(&format!(
            "  {} {} [{}]  {}\n",
            indicator,
            f.id,
            type_tag(&f.feature_type),
            truncate(&f.description, 50)
        ));
        if !f.depends_on.is_empty() {
            out.push_str(&format!("    \u{2190} {}\n", f.depends_on.join(", ")));
        }
    }

    for f in &pending_blocked_deps {
        out.push_str(&format!(
            "  \u{00B7} {} [{}]  {}\n",
            f.id,
            type_tag(&f.feature_type),
            truncate(&f.description, 50)
        ));
        if !f.depends_on.is_empty() {
            out.push_str(&format!("    \u{2190} {}\n", f.depends_on.join(", ")));
        }
    }

    for f in &blocked {
        let reason = f.blocked_reason.as_deref().unwrap_or("");
        out.push_str(&format!(
            "  \u{2717} {} [{}]  {}\n",
            f.id,
            type_tag(&f.feature_type),
            truncate(&f.description, 50)
        ));
        if !reason.is_empty() {
            out.push_str(&format!("    blocked: {reason}\n"));
        }
        if !f.depends_on.is_empty() {
            out.push_str(&format!("    \u{2190} {}\n", f.depends_on.join(", ")));
        }
    }

    // Next up — grouped by milestone
    if !milestone_groups.is_empty() {
        let has_claimable = milestone_groups.iter().any(|(_, ids)| !ids.is_empty());
        if has_claimable {
            out.push('\n');
            for (ms_id, ids) in &milestone_groups {
                if ids.is_empty() {
                    continue;
                }
                let top: Vec<&str> = ids.iter().take(3).copied().collect();
                if ms_id.is_empty() {
                    out.push_str(&format!("Next up: {}\n", top.join(", ")));
                } else {
                    out.push_str(&format!("Next up ({}): {}\n", ms_id, top.join(", ")));
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use features::{Feature, FeatureList, FeatureStatus, FeatureType};

    fn make_feature(id: &str, ft: FeatureType, desc: &str, deps: Vec<String>, priority: u32) -> Feature {
        Feature {
            id: id.into(),
            feature_type: ft,
            scope: "test".into(),
            description: desc.into(),
            verify: format!("./scripts/verify/{id}.sh"),
            depends_on: deps,
            priority,
            status: FeatureStatus::Pending,
            claimed_by: None,
            blocked_reason: None,
            context_hints: vec![],
        }
    }

    #[test]
    fn dag_empty_features() {
        let list = FeatureList { features: vec![] };
        let out = render_feature_dag(&list);
        assert!(out.contains("0 total"));
        assert!(out.contains("0 done"));
        // Should not panic
    }

    #[test]
    fn dag_all_done() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Create User struct", vec![], 1),
                make_feature("f002", FeatureType::Implement, "Add login endpoint", vec!["f001".into()], 2),
            ],
        };
        for f in &mut list.features {
            f.status = FeatureStatus::Done;
        }
        let out = render_feature_dag(&list);
        assert!(out.contains("2 done"));
        assert!(out.contains("Progress: 100%"));
        assert!(out.contains("\u{2713} f001"));
        assert!(out.contains("\u{2713} f002"));
        // No "Next up" when all done
        assert!(!out.contains("Next up"));
    }

    #[test]
    fn dag_mixed_states() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Create User struct", vec![], 1),
                make_feature("p001", FeatureType::Poc, "Validate thrift parsing", vec![], 1),
                make_feature("f002", FeatureType::Implement, "Add login endpoint", vec!["f001".into()], 2),
                make_feature("f003", FeatureType::Implement, "Data model boundaries", vec!["f001".into()], 3),
                make_feature("f004", FeatureType::Implement, "Add user validation", vec!["f002".into(), "f003".into()], 4),
            ],
        };
        // f001 done, p001 done, f002 claimed
        list.features[0].status = FeatureStatus::Done;
        list.features[1].status = FeatureStatus::Done;
        list.features[2].status = FeatureStatus::Claimed;
        list.features[2].claimed_by = Some("agent-1".into());

        let out = render_feature_dag(&list);
        // Header
        assert!(out.contains("5 total (2 done, 1 claimed, 2 pending)"));
        assert!(out.contains("Progress: 40%"));
        // Done features with checkmark
        assert!(out.contains("\u{2713} f001 [impl]"));
        assert!(out.contains("\u{2713} p001 [poc]"));
        // Claimed feature with hourglass
        assert!(out.contains("\u{29D7} f002 [impl]"));
        assert!(out.contains("(agent-1)"));
        // f003 is claimable (pending, f001 done)
        assert!(out.contains("\u{25B8} f003 [impl]"));
        // f004 pending with unmet deps
        assert!(out.contains("\u{00B7} f004 [impl]"));
        assert!(out.contains("\u{2190} f002, f003"));
        // Next up (no milestones so orphan list)
        assert!(out.contains("Next up: f003"));
    }

    #[test]
    fn dag_blocked_feature() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Create User struct", vec![], 1),
            ],
        };
        list.features[0].status = FeatureStatus::Blocked;
        list.features[0].blocked_reason = Some("stuck on compile error".into());

        let out = render_feature_dag(&list);
        assert!(out.contains("\u{2717} f001 [impl]"));
        assert!(out.contains("blocked: stuck on compile error"));
    }

    #[test]
    fn dag_truncates_long_description() {
        let list = FeatureList {
            features: vec![
                make_feature(
                    "f001",
                    FeatureType::Implement,
                    "This is a very long description that exceeds fifty characters and should be truncated",
                    vec![],
                    1,
                ),
            ],
        };
        let out = render_feature_dag(&list);
        // Description should be truncated to 50 chars with "..."
        assert!(out.contains("..."));
        // Should not contain the full description
        assert!(!out.contains("should be truncated"));
    }

    #[test]
    fn dag_milestone_priority() {
        // M3 (r103) depends on f031, f032
        // M4 (r104) depends on f036, f042, f043
        // f042 has lower raw priority than f099, but f042 blocks M4 (earlier milestone)
        let mut list = FeatureList {
            features: vec![
                // Done deps
                make_feature("f030", FeatureType::Implement, "Fragment builder", vec![], 50),
                make_feature("f035", FeatureType::Implement, "Optimizer", vec![], 51),
                // Claimable: blocks M4
                make_feature("f042", FeatureType::Implement, "UNION/INTERSECT", vec!["f035".into()], 139),
                make_feature("f043", FeatureType::Implement, "INSERT", vec!["f030".into()], 140),
                // Claimable: blocks M5 (lower raw priority number but later milestone)
                make_feature("f065", FeatureType::Implement, "Role management", vec![], 100),
                // Orphan claimable (no milestone)
                make_feature("f099", FeatureType::Implement, "Misc feature", vec![], 1),
                // Milestones
                make_feature("r104", FeatureType::Review, "M4 review", vec!["f042".into(), "f043".into()], 154),
                make_feature("r105", FeatureType::Review, "M5 review", vec!["f065".into()], 179),
            ],
        };
        list.features[0].status = FeatureStatus::Done; // f030
        list.features[1].status = FeatureStatus::Done; // f035

        let out = render_feature_dag(&list);

        // M4 features shown first in "Next up", labeled with milestone
        assert!(out.contains("Next up (r104): f042, f043"));
        // M5 features shown separately
        assert!(out.contains("Next up (r105): f065"));
        // Orphan shown without milestone label
        assert!(out.contains("Next up: f099"));

        // Top 3 get ▸: f042, f043 (M4), f065 (M5)
        assert!(out.contains("\u{25B8} f042"));
        assert!(out.contains("\u{25B8} f043"));
        assert!(out.contains("\u{25B8} f065"));
        // f099 is 4th, gets · (orphan, not in top 3)
        assert!(out.contains("\u{00B7} f099"));

        // Claimable section ordered by milestone: M4 features before M5 before orphans
        let pos_042 = out.find("\u{25B8} f042").unwrap();
        let pos_043 = out.find("\u{25B8} f043").unwrap();
        let pos_065 = out.find("\u{25B8} f065").unwrap();
        let pos_099 = out.find("\u{00B7} f099").unwrap();
        assert!(pos_042 < pos_043, "f042 before f043 (same milestone, lower priority)");
        assert!(pos_043 < pos_065, "M4 features before M5 features");
        assert!(pos_065 < pos_099, "milestone features before orphans");
    }

    #[test]
    fn dag_transitive_milestone_deps() {
        // r104 depends on f044, f044 depends on f043 (transitive)
        // f043 is claimable and should be surfaced as blocking r104
        let mut list = FeatureList {
            features: vec![
                make_feature("f030", FeatureType::Implement, "Done dep", vec![], 50),
                make_feature("f043", FeatureType::Implement, "INSERT", vec!["f030".into()], 140),
                make_feature("f044", FeatureType::Implement, "Stream load", vec!["f043".into()], 141),
                make_feature("r104", FeatureType::Review, "M4 review", vec!["f044".into()], 154),
            ],
        };
        list.features[0].status = FeatureStatus::Done; // f030

        let out = render_feature_dag(&list);
        // f043 is claimable and transitively blocks r104
        assert!(out.contains("Next up (r104): f043"));
        assert!(out.contains("\u{25B8} f043"));
    }
}

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
    use features::{FeatureList, FeatureStatus, FeatureType};
    use std::collections::HashMap;

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

    let feature_map: HashMap<&str, &features::Feature> =
        features.features.iter().map(|f| (f.id.as_str(), f)).collect();

    // === Milestones: only review features with M\d+ labels ===
    let mut milestones: Vec<&features::Feature> = features
        .features
        .iter()
        .filter(|f| f.feature_type == FeatureType::Review)
        .filter(|f| {
            let label = FeatureList::milestone_label(f);
            label.starts_with('M')
        })
        .collect();
    milestones.sort_by_key(|f| {
        let label = FeatureList::milestone_label(f);
        FeatureList::milestone_sort_key(&label)
    });

    // Extract a short description from a milestone's full description.
    // Tries: (1) name between "M\d+" label and "review" (e.g. "Foundation"),
    //        (2) gate description after "review." until ":" or "." (e.g. "Docker gate").
    let milestone_desc = |desc: &str, label: &str| -> String {
        // Strategy 1: text between label and "review"
        if let Some(start) = desc.find(label) {
            let after_label = &desc[start + label.len()..];
            let trimmed = after_label.trim();
            if let Some(pos) = trimmed.to_lowercase().find("review") {
                let name = trimmed[..pos].trim();
                if !name.is_empty() && name.to_lowercase() != "milestone" {
                    return name.to_string();
                }
            }
        }
        // Strategy 2: gate description after "review."
        if let Some(pos) = desc.to_lowercase().find("review") {
            let after = &desc[pos + 6..];
            let rest = after.trim_start_matches(|c: char| c == '.' || c == ',' || c.is_whitespace());
            if !rest.is_empty() {
                let end = rest.find(|c: char| c == ':' || c == '.')
                    .unwrap_or(rest.len())
                    .min(30);
                let short = rest[..end].trim();
                if !short.is_empty() {
                    return short.to_string();
                }
            }
        }
        String::new()
    };

    if !milestones.is_empty() {
        out.push_str("\nMilestones:\n");
        for ms in &milestones {
            let label = FeatureList::milestone_label(ms);
            let total = ms.depends_on.len();
            let done_count = ms
                .depends_on
                .iter()
                .filter(|dep| {
                    feature_map
                        .get(dep.as_str())
                        .is_some_and(|f| f.status == FeatureStatus::Done)
                })
                .count();
            let wip_count = ms
                .depends_on
                .iter()
                .filter(|dep| {
                    feature_map
                        .get(dep.as_str())
                        .is_some_and(|f| f.status == FeatureStatus::Claimed)
                })
                .count();

            let indicator = if ms.status == FeatureStatus::Done {
                "\u{2713}" // ✓
            } else if done_count + wip_count > 0 {
                "\u{25D0}" // ◐
            } else {
                "\u{00B7}" // ·
            };

            let short_desc = milestone_desc(&ms.description, &label);
            let ratio = format!("{done_count}/{total}");
            let mut line = format!("  {} {:<6} {:>5}", indicator, label, ratio);
            if !short_desc.is_empty() {
                line.push_str(&format!("  {short_desc}"));
            }
            if wip_count > 0 {
                line.push_str(&format!("  ({wip_count} wip)"));
            }
            line.push('\n');
            out.push_str(&line);
        }
    }

    // === In progress (claimed features) ===
    let truncate = |s: &str, max: usize| -> String {
        if s.len() <= max {
            s.to_string()
        } else {
            format!("{}...", &s[..max - 3])
        }
    };

    let mut claimed: Vec<&features::Feature> = features
        .features
        .iter()
        .filter(|f| f.status == FeatureStatus::Claimed)
        .collect();
    claimed.sort_by_key(|f| f.priority);

    if !claimed.is_empty() {
        out.push_str("\nIn progress:\n");
        for f in &claimed {
            let agent = f.claimed_by.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "  \u{29D7} {}  {}  ({})\n",
                f.id,
                truncate(&f.description, 45),
                agent
            ));
        }
    }

    // === Blocked features ===
    let mut blocked: Vec<&features::Feature> = features
        .features
        .iter()
        .filter(|f| f.status == FeatureStatus::Blocked)
        .collect();
    blocked.sort_by_key(|f| f.priority);

    if !blocked.is_empty() {
        out.push_str("\nBlocked:\n");
        for f in &blocked {
            let reason = f.blocked_reason.as_deref().unwrap_or("");
            out.push_str(&format!(
                "  \u{2717} {}  {}\n",
                f.id,
                truncate(&f.description, 45),
            ));
            if !reason.is_empty() {
                out.push_str(&format!("    reason: {reason}\n"));
            }
        }
    }

    // === Next up (grouped by milestone) ===
    let milestone_groups = features.milestone_claimable();
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
        assert!(!out.contains("Next up"));
        assert!(!out.contains("In progress"));
    }

    #[test]
    fn dag_mixed_states() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Create User struct", vec![], 1),
                make_feature("f002", FeatureType::Implement, "Add login endpoint", vec!["f001".into()], 2),
                make_feature("f003", FeatureType::Implement, "Data model boundaries", vec!["f001".into()], 3),
                make_feature("f004", FeatureType::Implement, "Add user validation", vec!["f002".into(), "f003".into()], 4),
            ],
        };
        list.features[0].status = FeatureStatus::Done;
        list.features[1].status = FeatureStatus::Claimed;
        list.features[1].claimed_by = Some("agent-1".into());

        let out = render_feature_dag(&list);
        assert!(out.contains("4 total (1 done, 1 claimed, 2 pending)"));
        assert!(out.contains("Progress: 25%"));
        // Claimed in "In progress" section
        assert!(out.contains("\u{29D7} f002"));
        assert!(out.contains("(agent-1)"));
        // f003 is claimable → shows in "Next up"
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
        assert!(out.contains("\u{2717} f001"));
        assert!(out.contains("reason: stuck on compile error"));
    }

    #[test]
    fn dag_truncates_long_description() {
        let mut list = FeatureList {
            features: vec![
                make_feature(
                    "f001",
                    FeatureType::Implement,
                    "This is a very long description that exceeds forty-five characters and should be truncated",
                    vec![],
                    1,
                ),
            ],
        };
        // Must be claimed or blocked to show individual description
        list.features[0].status = FeatureStatus::Claimed;
        list.features[0].claimed_by = Some("agent-1".into());
        let out = render_feature_dag(&list);
        assert!(out.contains("..."));
        assert!(!out.contains("should be truncated"));
    }

    #[test]
    fn dag_milestone_summary() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Feature A", vec![], 1),
                make_feature("f002", FeatureType::Implement, "Feature B", vec![], 2),
                make_feature("f003", FeatureType::Implement, "Feature C", vec!["f001".into()], 3),
                make_feature("f004", FeatureType::Implement, "Feature D", vec![], 4),
                make_feature("r101", FeatureType::Review, "M1 Foundation review. Verify scaffold", vec!["f001".into(), "f002".into()], 10),
                make_feature("r104", FeatureType::Review, "M4 milestone review. Docker oracle gate: run tests", vec!["f003".into(), "f004".into()], 20),
            ],
        };
        list.features[0].status = FeatureStatus::Done; // f001
        list.features[1].status = FeatureStatus::Done; // f002
        list.features[3].status = FeatureStatus::Done; // f004
        list.features[4].status = FeatureStatus::Done; // r101

        let out = render_feature_dag(&list);
        // M1 done: 2/2 with description
        assert!(out.contains("\u{2713}") && out.contains("M1"), "M1 should show done: {out}");
        assert!(out.contains("2/2"), "M1 should show 2/2: {out}");
        assert!(out.contains("Foundation"), "M1 should show description: {out}");
        // M4 partial: 1/2, gate description extracted
        assert!(out.contains("\u{25D0}"), "M4 should show partial: {out}");
        assert!(out.contains("1/2"), "M4 should show 1/2: {out}");
        assert!(out.contains("Docker oracle gate"), "M4 should show gate desc: {out}");
    }

    #[test]
    fn dag_milestone_wip_count() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "A", vec![], 1),
                make_feature("f002", FeatureType::Implement, "B", vec![], 2),
                make_feature("f003", FeatureType::Implement, "C", vec![], 3),
                make_feature("r104", FeatureType::Review, "M4 review", vec!["f001".into(), "f002".into(), "f003".into()], 10),
            ],
        };
        list.features[0].status = FeatureStatus::Done;
        list.features[1].status = FeatureStatus::Claimed;
        list.features[1].claimed_by = Some("agent-1".into());

        let out = render_feature_dag(&list);
        assert!(out.contains("1/3"), "M4 should show 1 done out of 3: {out}");
        assert!(out.contains("1 wip"), "M4 should show 1 wip: {out}");
        assert!(out.contains("\u{25D0}"), "M4 should show partial indicator: {out}");
    }

    #[test]
    fn dag_milestone_priority() {
        let mut list = FeatureList {
            features: vec![
                make_feature("f030", FeatureType::Implement, "Fragment builder", vec![], 50),
                make_feature("f035", FeatureType::Implement, "Optimizer", vec![], 51),
                make_feature("f042", FeatureType::Implement, "UNION/INTERSECT", vec!["f035".into()], 139),
                make_feature("f043", FeatureType::Implement, "INSERT", vec!["f030".into()], 140),
                make_feature("f065", FeatureType::Implement, "Role management", vec![], 100),
                make_feature("f099", FeatureType::Implement, "Misc feature", vec![], 1),
                make_feature("r104", FeatureType::Review, "M4 review", vec!["f042".into(), "f043".into()], 154),
                make_feature("r105", FeatureType::Review, "M5 review", vec!["f065".into()], 179),
            ],
        };
        list.features[0].status = FeatureStatus::Done; // f030
        list.features[1].status = FeatureStatus::Done; // f035

        let out = render_feature_dag(&list);

        // Milestone summary shows both
        assert!(out.contains("M4"), "Should show M4: {out}");
        assert!(out.contains("M5"), "Should show M5: {out}");
        // Next up grouped by milestone
        assert!(out.contains("Next up (M4): f042, f043"), "M4 next up: {out}");
        assert!(out.contains("Next up (M5): f065"), "M5 next up: {out}");
        assert!(out.contains("Next up: f099"), "Orphan next up: {out}");
        // M4 before M5
        let pos_m4 = out.find("Next up (M4)").unwrap();
        let pos_m5 = out.find("Next up (M5)").unwrap();
        assert!(pos_m4 < pos_m5, "M4 before M5 in next up");
    }

    #[test]
    fn dag_transitive_milestone_deps() {
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
        assert!(out.contains("Next up (M4): f043"), "Transitive dep in next up: {out}");
    }

    #[test]
    fn dag_no_milestone_labels_skips_section() {
        // Review features without M\d+ labels should not show in Milestones section
        let mut list = FeatureList {
            features: vec![
                make_feature("f001", FeatureType::Implement, "Feature A", vec![], 1),
                make_feature("r001", FeatureType::Review, "Review p001 results", vec!["f001".into()], 10),
            ],
        };
        list.features[0].status = FeatureStatus::Done;
        list.features[1].status = FeatureStatus::Done;

        let out = render_feature_dag(&list);
        assert!(!out.contains("Milestones:"), "No milestone section for non-M reviews: {out}");
    }
}

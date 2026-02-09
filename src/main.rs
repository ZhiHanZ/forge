mod config;
mod context;
mod features;
mod git;
mod init;
mod runner;
mod skills;
mod template;
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
    },
    /// Run all verify scripts
    Verify,
    /// Show project status: features, context, progress
    Status,
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { description } => cmd_init(&cli.project, &description),
        Commands::Run {
            agents,
            max_sessions,
            watch: _,
        } => cmd_run(&cli.project, agents, max_sessions),
        Commands::Verify => cmd_verify(&cli.project),
        Commands::Status => cmd_status(&cli.project),
        Commands::Stop => cmd_stop(&cli.project),
        Commands::Logs { agent, tail } => cmd_logs(&cli.project, &agent, tail),
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

fn cmd_run(project_dir: &PathBuf, agents: usize, max_sessions: usize) {
    // Load forge config to get role settings
    let forge_config = config::ForgeConfig::load(project_dir).unwrap_or_else(|_| {
        config::ForgeConfig::scaffold("unknown", "")
    });

    let protocol = forge_config.forge.roles.protocol.clone();
    let orchestrating = forge_config.forge.roles.orchestrating.clone();

    println!(
        "forge run: {} agent(s), backend={}, model={}, max_sessions={}",
        agents, protocol.backend, protocol.model, max_sessions
    );
    println!();

    let run_config = runner::RunConfig {
        project_dir: project_dir.clone(),
        protocol,
        orchestrating,
        max_sessions,
        num_agents: agents,
    };

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

fn cmd_status(project_dir: &PathBuf) {
    // Load features
    let features = match features::FeatureList::load(project_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error loading features: {e}");
            std::process::exit(1);
        }
    };

    let counts = features.status_counts();

    println!("Features: {} total", counts.total);
    println!(
        "  {} done, {} pending, {} claimed, {} blocked",
        counts.done, counts.pending, counts.claimed, counts.blocked
    );

    if counts.total > 0 {
        let pct = (counts.done as f64 / counts.total as f64) * 100.0;
        println!("  Progress: {pct:.0}%");
    }

    // Load context
    let ctx = context::ContextManager::new(project_dir);
    match ctx.counts() {
        Ok(ctx_counts) => {
            let total: usize = ctx_counts.values().sum();
            if total > 0 {
                println!();
                println!("Context: {total} entries");
                for (cat, count) in &ctx_counts {
                    if *count > 0 {
                        println!("  {cat}: {count}");
                    }
                }
            }
        }
        Err(_) => {}
    }

    // Show blocked features
    let blocked: Vec<_> = features
        .features
        .iter()
        .filter(|f| f.status == features::FeatureStatus::Blocked)
        .collect();

    if !blocked.is_empty() {
        println!();
        println!("Blocked features:");
        for f in blocked {
            let reason = f.blocked_reason.as_deref().unwrap_or("no reason given");
            println!("  {} — {reason}", f.id);
        }
    }
}

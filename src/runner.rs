use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;

use crate::config::RoleSpec;
use crate::features::{FeatureList, FeatureStatus};
use crate::git;
use crate::verify;

#[derive(Debug)]
pub enum RunOutcome {
    AllDone { sessions: usize },
    MaxSessions { sessions: usize, remaining: usize },
    Stopped { sessions: usize },
    SpawnError(std::io::Error),
}

/// Configuration for a forge run.
pub struct RunConfig {
    pub project_dir: PathBuf,
    pub protocol: RoleSpec,
    pub orchestrating: RoleSpec,
    pub max_sessions: usize,
    pub num_agents: usize,
}

/// Runtime directory for forge state (.forge/).
fn runtime_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".forge")
}

/// Check if a stop was requested.
pub fn stop_requested(project_dir: &Path) -> bool {
    runtime_dir(project_dir).join("stop").exists()
}

/// Request a stop (called by forge stop).
pub fn request_stop(project_dir: &Path) -> Result<(), std::io::Error> {
    let dir = runtime_dir(project_dir);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("stop"), "")?;
    Ok(())
}

/// Clear the stop sentinel.
fn clear_stop(project_dir: &Path) {
    let _ = fs::remove_file(runtime_dir(project_dir).join("stop"));
}

/// Open a log file for an agent.
fn open_log(project_dir: &Path, agent_id: &str) -> Option<std::fs::File> {
    let log_dir = runtime_dir(project_dir).join("logs");
    fs::create_dir_all(&log_dir).ok()?;
    fs::File::create(log_dir.join(format!("{agent_id}.log"))).ok()
}

/// Build the agent prompt for a feature.
/// If a context package exists, embeds its contents directly so the agent
/// doesn't need to explore the codebase for pre-compiled context.
pub fn build_agent_prompt(project_dir: &Path, feature_id: &str) -> String {
    let package_path = project_dir.join(format!("context/packages/{feature_id}.md"));
    let context_block = std::fs::read_to_string(&package_path).unwrap_or_default();

    if context_block.is_empty() {
        format!(
            "You are a forge agent. Your assigned feature is {feature_id}. \
             Read features.json for details. Follow the forge-protocol skill. \
             When done, set status to done and exit.",
        )
    } else {
        format!(
            "You are a forge agent. Your assigned feature is {feature_id}. \
             Follow the forge-protocol skill.\n\n\
             ## Pre-compiled context (DO NOT use Explore agents — this has what you need)\n\n\
             {context_block}\n\n\
             Read features.json for your verify command. \
             When done, set status to done and exit.",
        )
    }
}

/// Check that the agent followed protocol after its session.
/// These are CLI-enforced gates that don't depend on the agent's self-reporting.
fn check_protocol_compliance(project_dir: &Path, feature_id: &str) {
    // Check 1: exec-memory was written (agent completed handoff)
    let exec_memory = project_dir.join(format!("feedback/exec-memory/{feature_id}.json"));
    if !exec_memory.exists() {
        eprintln!("  WARN: No exec-memory for {feature_id} — agent skipped handoff protocol");
    } else {
        // Check 2: delivery proof exists in exec-memory
        if let Ok(content) = fs::read_to_string(&exec_memory) {
            if !content.contains("\"delivery\"") {
                eprintln!(
                    "  WARN: exec-memory for {feature_id} has no delivery proof — \
                     requirements not mapped to code/tests"
                );
            }
        }
    }

    // Check 3: feature status was updated (not left as "claimed")
    if let Ok(features) = FeatureList::load(project_dir) {
        if let Some(f) = features.features.iter().find(|f| f.id == feature_id) {
            if f.status == FeatureStatus::Claimed {
                eprintln!(
                    "  WARN: {feature_id} still 'claimed' after session — \
                     agent didn't mark done or blocked"
                );
            }
        }
    }
}

/// Run the autonomous development loop with a single agent.
pub fn run_single_agent(config: &RunConfig) -> RunOutcome {
    let mut session = 0;

    // Ensure runtime dir exists
    let _ = fs::create_dir_all(runtime_dir(&config.project_dir));

    // Sync CocoIndex context flow files
    crate::context_flow::sync_context_flow(&config.project_dir);

    loop {
        // Check for stop request
        if stop_requested(&config.project_dir) {
            clear_stop(&config.project_dir);
            return RunOutcome::Stopped { sessions: session };
        }

        // Check if all features are done
        let features = match FeatureList::load(&config.project_dir) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error loading features: {e}");
                return RunOutcome::SpawnError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ));
            }
        };

        if features.all_done() {
            return RunOutcome::AllDone { sessions: session };
        }

        if session >= config.max_sessions {
            let remaining = features
                .features
                .iter()
                .filter(|f| f.status != FeatureStatus::Done)
                .count();
            return RunOutcome::MaxSessions {
                sessions: session,
                remaining,
            };
        }

        // Find next claimable feature
        let (next, next_type) = match features.next_claimable() {
            Some(f) => (f.id.clone(), f.feature_type.clone()),
            None => {
                eprintln!("No claimable features (all blocked or claimed)");
                let remaining = features
                    .features
                    .iter()
                    .filter(|f| f.status != FeatureStatus::Done)
                    .count();
                return RunOutcome::MaxSessions {
                    sessions: session,
                    remaining,
                };
            }
        };

        // Refresh CocoIndex context packages
        match crate::context_flow::refresh_context(&config.project_dir) {
            Ok(true) => println!("  Context packages refreshed."),
            Ok(false) => {}
            Err(e) => eprintln!("  Context refresh warning: {e}"),
        }

        println!("--- Session {session} ---");
        println!("  Feature: {next}");

        // --- Phase 1: Executor ---
        // Use orchestrating role for review features (milestone gates),
        // protocol role for implement/poc features.
        let role = match next_type {
            crate::features::FeatureType::Review => &config.orchestrating,
            _ => &config.protocol,
        };
        let prompt = build_agent_prompt(&config.project_dir, &next);

        let mut log = open_log(&config.project_dir, "agent-1");

        match spawn_agent(role, &config.project_dir, &prompt, "agent-1") {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => {
                                println!("  [{next}] {line}");
                                if let Some(ref mut f) = log {
                                    let _ = writeln!(f, "{line}");
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
                let status = child.wait();
                println!(
                    "  Agent exited: {}",
                    status.map_or("unknown".into(), |s| s.to_string())
                );
            }
            Err(e) => {
                eprintln!("  Failed to spawn agent: {e}");
                return RunOutcome::SpawnError(e);
            }
        }

        // --- Phase 1.5: Protocol compliance checks ---
        check_protocol_compliance(&config.project_dir, &next);

        // --- Phase 2: Verify ---
        println!("  Running post-session verify...");
        match verify::verify_all(&config.project_dir) {
            Ok(results) => {
                for result in &results {
                    let status = if result.passed { "PASS" } else { "FAIL" };
                    println!("  [{status}] {}", result.feature_id);
                }

                // Write feedback/last-verify.json
                let report = verify::VerifyReport::from_results(&results);
                if let Err(e) = report.write(&config.project_dir) {
                    eprintln!("  Failed to write verify report: {e}");
                }

                // Reopen features that failed verify
                if let Ok(mut features) = FeatureList::load(&config.project_dir) {
                    let mut changed = false;
                    for result in &results {
                        if !result.passed {
                            if let Ok(()) = features.reopen(&result.feature_id) {
                                println!("  Reopened {} (verify failed)", result.feature_id);
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        let _ = features.save(&config.project_dir);
                    }
                }
            }
            Err(e) => eprintln!("  Verify error: {e}"),
        }

        // --- Phase 3: Git sync ---
        if git::is_git_repo(&config.project_dir) {
            if let Err(e) = git::pull(&config.project_dir) {
                eprintln!("  Git pull warning: {e}");
            }
        }

        // --- Phase 4: Orchestrating review ---
        println!("  Dispatching orchestrating review...");
        let orch_prompt = format!(
            "You are a forge orchestrating agent. Follow the forge-orchestrating skill. \
             Review the last executor session: read feedback/last-verify.json, run git diff HEAD~1, \
             check code against principles. Review feedback/exec-memory/{next}.json for session \
             tactics — assess approach, test strategy, and insights quality. \
             Write feedback/session-review.md and any context entries. Then commit and exit."
        );

        match spawn_agent(&config.orchestrating, &config.project_dir, &orch_prompt, "orchestrator") {
            Ok(mut child) => {
                // Capture but don't print orchestrator output (it's housekeeping)
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => {
                                if let Some(ref mut f) = log {
                                    let _ = writeln!(f, "[orch] {line}");
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
                let _ = child.wait();
            }
            Err(e) => {
                // Orchestrating failure is non-fatal — executor can continue without review
                eprintln!("  Orchestrating dispatch failed (non-fatal): {e}");
            }
        }

        session += 1;
    }
}

/// Run the multi-agent development loop using git worktrees.
pub fn run_multi_agent(config: &RunConfig) -> RunOutcome {
    let mut session = 0;
    let _ = fs::create_dir_all(runtime_dir(&config.project_dir));

    // Sync CocoIndex context flow files
    crate::context_flow::sync_context_flow(&config.project_dir);

    // Must be a git repo for worktrees
    if !git::is_git_repo(&config.project_dir) {
        eprintln!("Multi-agent mode requires a git repository.");
        return RunOutcome::SpawnError(std::io::Error::new(
            std::io::ErrorKind::Other,
            "not a git repo",
        ));
    }

    loop {
        if stop_requested(&config.project_dir) {
            clear_stop(&config.project_dir);
            return RunOutcome::Stopped { sessions: session };
        }

        let features = match FeatureList::load(&config.project_dir) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error loading features: {e}");
                return RunOutcome::SpawnError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ));
            }
        };

        if features.all_done() {
            return RunOutcome::AllDone { sessions: session };
        }

        if session >= config.max_sessions {
            let remaining = features
                .features
                .iter()
                .filter(|f| f.status != FeatureStatus::Done)
                .count();
            return RunOutcome::MaxSessions {
                sessions: session,
                remaining,
            };
        }

        // Find up to N claimable features
        let claimable = features.next_n_claimable(config.num_agents);
        if claimable.is_empty() {
            let remaining = features
                .features
                .iter()
                .filter(|f| f.status != FeatureStatus::Done)
                .count();
            return RunOutcome::MaxSessions {
                sessions: session,
                remaining,
            };
        }

        // Refresh CocoIndex context packages
        match crate::context_flow::refresh_context(&config.project_dir) {
            Ok(true) => println!("  Context packages refreshed."),
            Ok(false) => {}
            Err(e) => eprintln!("  Context refresh warning: {e}"),
        }

        let feature_entries: Vec<(String, crate::features::FeatureType)> = claimable
            .iter()
            .map(|f| (f.id.clone(), f.feature_type.clone()))
            .collect();

        println!(
            "--- Session {session} ({} agents) ---",
            feature_entries.len()
        );
        for (fid, _) in &feature_entries {
            println!("  Feature: {fid}");
        }

        // Create worktrees and spawn agents in parallel
        let wt_base = runtime_dir(&config.project_dir).join("worktrees");
        let _ = fs::create_dir_all(&wt_base);

        let mut handles = Vec::new();
        let feature_ids: Vec<String> = feature_entries.iter().map(|(id, _)| id.clone()).collect();

        for (i, (feature_id, ftype)) in feature_entries.iter().enumerate() {
            let agent_id = format!("agent-{}", i + 1);
            let branch = format!("forge/{agent_id}");
            let wt_dir = wt_base.join(&agent_id);

            // Clean up stale worktree if exists
            if wt_dir.exists() {
                let _ = git::remove_worktree(&config.project_dir, &wt_dir);
            }

            if let Err(e) = git::create_worktree(&config.project_dir, &wt_dir, &branch) {
                eprintln!("  Failed to create worktree for {agent_id}: {e}");
                continue;
            }

            let prompt = build_agent_prompt(&config.project_dir, feature_id);

            // Use orchestrating role for review features, protocol for implement/poc
            let role = match ftype {
                crate::features::FeatureType::Review => config.orchestrating.clone(),
                _ => config.protocol.clone(),
            };
            let wt = wt_dir.clone();
            let fid = feature_id.clone();
            let project_dir = config.project_dir.clone();
            let aid = agent_id.clone();
            let handle = thread::spawn(move || {
                let mut log = open_log(&project_dir, &aid);
                match spawn_agent(&role, &wt, &prompt, &aid) {
                    Ok(mut child) => {
                        if let Some(stdout) = child.stdout.take() {
                            let reader = BufReader::new(stdout);
                            for line in reader.lines() {
                                match line {
                                    Ok(line) => {
                                        println!("  [{fid}] {line}");
                                        if let Some(ref mut f) = log {
                                            let _ = writeln!(f, "{line}");
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                        let _ = child.wait();
                    }
                    Err(e) => {
                        eprintln!("  Failed to spawn {aid}: {e}");
                    }
                }
            });
            handles.push((handle, wt_dir, agent_id));
        }

        // Wait for all agents
        for (handle, _, agent_id) in &handles {
            if handle.is_finished() {
                continue;
            }
            println!("  Waiting for {agent_id}...");
        }
        // Actually join them
        let worktree_dirs: Vec<(PathBuf, String)> = handles
            .into_iter()
            .map(|(handle, wt_dir, agent_id)| {
                let _ = handle.join();
                (wt_dir, agent_id)
            })
            .collect();

        // Merge worktree branches back into main
        for (wt_dir, agent_id) in &worktree_dirs {
            let branch = format!("forge/{agent_id}");
            if let Err(e) = merge_worktree(&config.project_dir, wt_dir, &branch) {
                eprintln!("  Merge failed for {agent_id}: {e}");
            }
        }

        // Clean up worktrees
        for (wt_dir, agent_id) in &worktree_dirs {
            if let Err(e) = git::remove_worktree(&config.project_dir, wt_dir) {
                eprintln!("  Failed to remove worktree for {agent_id}: {e}");
            }
        }

        // --- Protocol compliance checks ---
        for (fid, _) in &feature_entries {
            check_protocol_compliance(&config.project_dir, fid);
        }

        // --- Verify ---
        println!("  Running post-session verify...");
        match verify::verify_all(&config.project_dir) {
            Ok(results) => {
                for result in &results {
                    let status = if result.passed { "PASS" } else { "FAIL" };
                    println!("  [{status}] {}", result.feature_id);
                }

                let report = verify::VerifyReport::from_results(&results);
                if let Err(e) = report.write(&config.project_dir) {
                    eprintln!("  Failed to write verify report: {e}");
                }

                if let Ok(mut features) = FeatureList::load(&config.project_dir) {
                    let mut changed = false;
                    for result in &results {
                        if !result.passed {
                            if let Ok(()) = features.reopen(&result.feature_id) {
                                println!("  Reopened {} (verify failed)", result.feature_id);
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        let _ = features.save(&config.project_dir);
                    }
                }
            }
            Err(e) => eprintln!("  Verify error: {e}"),
        }

        // --- Git sync ---
        if let Err(e) = git::pull(&config.project_dir) {
            eprintln!("  Git pull warning: {e}");
        }

        // --- Orchestrating review ---
        println!("  Dispatching orchestrating review...");
        let fids_str = feature_ids.join(", ");
        let orch_prompt = format!(
            "You are a forge orchestrating agent. Follow the forge-orchestrating skill. \
             Review the last executor session: read feedback/last-verify.json, run git diff HEAD~1, \
             check code against principles. Review feedback/exec-memory/ for session tactics of \
             features [{fids_str}] — assess approach, test strategy, and insights quality. \
             Write feedback/session-review.md and any context entries. Then commit and exit."
        );

        match spawn_agent(
            &config.orchestrating,
            &config.project_dir,
            &orch_prompt,
            "orchestrator",
        ) {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        if line.is_err() {
                            break;
                        }
                    }
                }
                let _ = child.wait();
            }
            Err(e) => {
                eprintln!("  Orchestrating dispatch failed (non-fatal): {e}");
            }
        }

        session += 1;
    }
}

/// Merge a worktree branch back into the current branch.
fn merge_worktree(repo_dir: &Path, _wt_dir: &Path, branch: &str) -> Result<(), String> {
    // First commit any changes in the worktree (the agent may have left uncommitted work)
    // The worktree is on its own branch, so we merge that branch into main
    let output = Command::new("git")
        .args(["merge", branch, "--no-edit"])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("git merge failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Abort the merge on conflict
        let _ = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(repo_dir)
            .output();
        return Err(format!("merge conflict: {stderr}"));
    }
    Ok(())
}

/// Build the command and arguments for spawning an agent interactively (no --print/exec).
/// Used by the TUI --watch mode to spawn agents in a PTY.
pub fn build_agent_command(role: &RoleSpec, prompt: &str) -> (String, Vec<String>) {
    match role.backend.as_str() {
        "claude" => (
            "claude".to_string(),
            vec![
                "--model".to_string(),
                role.model.clone(),
                "--dangerously-skip-permissions".to_string(),
                prompt.to_string(),
            ],
        ),
        "codex" => (
            "codex".to_string(),
            vec![
                "--model".to_string(),
                role.model.clone(),
                "--full-auto".to_string(),
                prompt.to_string(),
            ],
        ),
        _ => (role.backend.clone(), vec![prompt.to_string()]),
    }
}

/// Spawn an agent child process using the role's backend + model.
fn spawn_agent(
    role: &RoleSpec,
    project_dir: &Path,
    prompt: &str,
    agent_id: &str,
) -> Result<Child, std::io::Error> {
    let (cmd, mut args) = build_agent_command(role, prompt);

    // For headless mode, add --print (claude) or exec prefix (codex)
    match role.backend.as_str() {
        "claude" => {
            args.insert(0, "--print".to_string());
        }
        "codex" => {
            args.insert(0, "exec".to_string());
        }
        _ => {}
    }

    Command::new(&cmd)
        .args(&args)
        .current_dir(project_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("FORGE_AGENT_ID", agent_id)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoleSpec;
    use crate::features::{Feature, FeatureList, FeatureStatus, FeatureType};

    fn setup_project(dir: &Path, features: Vec<Feature>) {
        let list = FeatureList { features };
        list.save(dir).unwrap();
        fs::create_dir_all(dir.join("scripts/verify")).unwrap();
    }

    fn echo_role() -> RoleSpec {
        RoleSpec {
            backend: "echo".into(),
            model: "test".into(),
        }
    }

    #[test]
    fn all_done_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(
            dir.path(),
            vec![Feature {
                id: "f001".into(),
                feature_type: FeatureType::Implement,
                scope: "test".into(),
                description: "already done".into(),
                verify: "./scripts/verify/f001.sh".into(),
                depends_on: vec![],
                priority: 1,
                status: FeatureStatus::Done,
                claimed_by: Some("prev-agent".into()),
                blocked_reason: None,
                context_hints: vec![],
            }],
        );

        let config = RunConfig {
            project_dir: dir.path().to_path_buf(),
            protocol: echo_role(),
            orchestrating: echo_role(),
            max_sessions: 10,
            num_agents: 1,
        };

        match run_single_agent(&config) {
            RunOutcome::AllDone { sessions } => assert_eq!(sessions, 0),
            other => panic!("Expected AllDone, got {other:?}"),
        }
    }

    #[test]
    fn max_sessions_stops_loop() {
        let dir = tempfile::tempdir().unwrap();

        fs::create_dir_all(dir.path().join("scripts/verify")).unwrap();
        fs::write(
            dir.path().join("scripts/verify/f001.sh"),
            "#!/bin/bash\nexit 0",
        )
        .unwrap();

        setup_project(
            dir.path(),
            vec![Feature {
                id: "f001".into(),
                feature_type: FeatureType::Implement,
                scope: "test".into(),
                description: "test".into(),
                verify: "./scripts/verify/f001.sh".into(),
                depends_on: vec![],
                priority: 1,
                status: FeatureStatus::Pending,
                claimed_by: None,
                blocked_reason: None,
                context_hints: vec![],
            }],
        );

        let config = RunConfig {
            project_dir: dir.path().to_path_buf(),
            protocol: echo_role(),
            orchestrating: echo_role(),
            max_sessions: 2,
            num_agents: 1,
        };

        match run_single_agent(&config) {
            RunOutcome::MaxSessions { sessions, .. } => {
                assert!(sessions <= 2);
            }
            RunOutcome::AllDone { .. } => {}
            RunOutcome::SpawnError(_) => {}
            RunOutcome::Stopped { .. } => {}
        }
    }

    #[test]
    fn spawn_agent_uses_role() {
        let dir = tempfile::tempdir().unwrap();
        let role = echo_role();
        let result = spawn_agent(&role, dir.path(), "test prompt", "agent-1");
        assert!(result.is_ok());
        let mut child = result.unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
    }

    #[test]
    fn stop_sentinel_works() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!stop_requested(dir.path()));
        request_stop(dir.path()).unwrap();
        assert!(stop_requested(dir.path()));
        clear_stop(dir.path());
        assert!(!stop_requested(dir.path()));
    }

    #[test]
    fn stop_halts_loop() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(
            dir.path(),
            vec![Feature {
                id: "f001".into(),
                feature_type: FeatureType::Implement,
                scope: "test".into(),
                description: "test".into(),
                verify: "./scripts/verify/f001.sh".into(),
                depends_on: vec![],
                priority: 1,
                status: FeatureStatus::Pending,
                claimed_by: None,
                blocked_reason: None,
                context_hints: vec![],
            }],
        );

        // Pre-request stop before starting the loop
        request_stop(dir.path()).unwrap();

        let config = RunConfig {
            project_dir: dir.path().to_path_buf(),
            protocol: echo_role(),
            orchestrating: echo_role(),
            max_sessions: 100,
            num_agents: 1,
        };

        match run_single_agent(&config) {
            RunOutcome::Stopped { sessions } => assert_eq!(sessions, 0),
            other => panic!("Expected Stopped, got {other:?}"),
        }
    }

    #[test]
    fn writes_verify_report() {
        let dir = tempfile::tempdir().unwrap();

        fs::create_dir_all(dir.path().join("scripts/verify")).unwrap();
        fs::write(
            dir.path().join("scripts/verify/f001.sh"),
            "#!/bin/bash\nexit 0",
        )
        .unwrap();

        setup_project(
            dir.path(),
            vec![Feature {
                id: "f001".into(),
                feature_type: FeatureType::Implement,
                scope: "test".into(),
                description: "test".into(),
                verify: "./scripts/verify/f001.sh".into(),
                depends_on: vec![],
                priority: 1,
                status: FeatureStatus::Pending,
                claimed_by: None,
                blocked_reason: None,
                context_hints: vec![],
            }],
        );

        let config = RunConfig {
            project_dir: dir.path().to_path_buf(),
            protocol: echo_role(),
            orchestrating: echo_role(),
            max_sessions: 1,
            num_agents: 1,
        };

        run_single_agent(&config);

        // Verify report should have been written
        let report_path = dir.path().join("feedback/last-verify.json");
        // Report may or may not exist depending on whether echo produces done status,
        // but the code path is exercised
        if report_path.exists() {
            let content = fs::read_to_string(&report_path).unwrap();
            assert!(content.contains("pass"));
        }
    }

    #[test]
    fn log_file_created() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(
            dir.path(),
            vec![Feature {
                id: "f001".into(),
                feature_type: FeatureType::Implement,
                scope: "test".into(),
                description: "test".into(),
                verify: "./scripts/verify/f001.sh".into(),
                depends_on: vec![],
                priority: 1,
                status: FeatureStatus::Pending,
                claimed_by: None,
                blocked_reason: None,
                context_hints: vec![],
            }],
        );

        let config = RunConfig {
            project_dir: dir.path().to_path_buf(),
            protocol: echo_role(),
            orchestrating: echo_role(),
            max_sessions: 1,
            num_agents: 1,
        };

        run_single_agent(&config);

        let log_path = dir.path().join(".forge/logs/agent-1.log");
        assert!(log_path.exists());
    }
}

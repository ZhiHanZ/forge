use crate::config::ForgeConfig;

/// Generate CLAUDE.md content from forge config (~40 lines).
pub fn generate_claude_md(config: &ForgeConfig) -> String {
    let mut lines = Vec::new();

    lines.push(format!("# {}", config.project.name));
    if !config.project.stack.is_empty() {
        lines.push(format!("Stack: {}", config.project.stack));
    }
    lines.push(String::new());

    lines.push("## Build & Test".into());
    lines.push(String::new());
    lines.push("- Build: `cargo build`".into());
    lines.push("- Test: `cargo test`".into());
    lines.push("- Lint: `cargo clippy`".into());
    lines.push("- Format: `cargo fmt`".into());
    lines.push(String::new());

    lines.push("## Principles (non-negotiable)".into());
    lines.push(String::new());
    if !config.principles.readability.is_empty() {
        lines.push(format!("1. {}", config.principles.readability));
    }
    if !config.principles.proof.is_empty() {
        lines.push(format!("2. {}", config.principles.proof));
    }
    if !config.principles.style.is_empty() {
        lines.push(format!("3. {}", config.principles.style));
    }
    if !config.principles.boundaries.is_empty() {
        lines.push(format!("4. {}", config.principles.boundaries));
    }
    lines.push(String::new());

    lines.push("## Forge Agent".into());
    lines.push(String::new());
    lines.push("You are in a managed development loop. Follow the forge-protocol skill.".into());
    lines.push(String::new());

    lines.push("### State (read first every session)".into());
    lines.push("- `context/INDEX.md` — scan one-liners to find relevant context.".into());
    lines.push("- `features.json` — task list. Find your work here.".into());
    lines.push("- `context/decisions/` — why choices were made.".into());
    lines.push("- `context/gotchas/` — known pitfalls.".into());
    lines.push("- `context/patterns/` — code conventions.".into());
    lines.push("- `context/poc/` — POC outcomes (goal, result, learnings, design impact).".into());
    lines.push("- `context/references/` — external knowledge, read instead of re-searching.".into());
    lines.push("- `feedback/session-review.md` — last session's review (read first!).".into());
    lines.push(String::new());

    lines.push("### Protocol".into());
    lines.push("1. Read features.json -> claim highest-priority unblocked pending feature".into());
    lines.push("2. Commit the claim. If push fails, pick another.".into());
    lines.push("3. Implement. Run the feature's `verify` command.".into());
    lines.push("4. Pass -> status \"done\". Fail -> fix and retry.".into());
    lines.push("5. Discoveries -> write to context/{decisions,gotchas,patterns}/".into());
    lines.push(
        "6. External knowledge (web search, blog, doc) -> write to context/references/".into(),
    );
    lines.push("7. Commit all. Push. Exit.".into());
    lines.push(String::new());

    lines.push("### Hard rules".into());
    lines.push("- One feature per session. Never scope-creep.".into());
    lines.push("- Never modify features you didn't claim.".into());
    lines.push("- Never weaken verify commands.".into());
    lines.push("- Stuck 10+ attempts -> status \"blocked\", add reason, exit.".into());

    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ForgeConfig;

    #[test]
    fn claude_md_has_project_name() {
        let config = ForgeConfig::scaffold("my-app", "Rust, axum");
        let md = generate_claude_md(&config);
        assert!(md.starts_with("# my-app\n"));
        assert!(md.contains("Stack: Rust, axum"));
    }

    #[test]
    fn claude_md_has_principles() {
        let config = ForgeConfig::scaffold("test", "Rust");
        let md = generate_claude_md(&config);
        assert!(md.contains("## Principles (non-negotiable)"));
        assert!(md.contains("1. Code understood in one read"));
    }

    #[test]
    fn claude_md_has_protocol() {
        let config = ForgeConfig::scaffold("test", "Rust");
        let md = generate_claude_md(&config);
        assert!(md.contains("### Protocol"));
        assert!(md.contains("Read features.json"));
        assert!(md.contains("context/references/"));
    }

    #[test]
    fn claude_md_has_poc_context() {
        let config = ForgeConfig::scaffold("test", "Rust");
        let md = generate_claude_md(&config);
        assert!(md.contains("context/poc/"));
    }

    #[test]
    fn claude_md_has_hard_rules() {
        let config = ForgeConfig::scaffold("test", "Rust");
        let md = generate_claude_md(&config);
        assert!(md.contains("### Hard rules"));
        assert!(md.contains("One feature per session"));
        assert!(md.contains("Never weaken verify"));
    }

    #[test]
    fn claude_md_under_45_lines() {
        let config = ForgeConfig::scaffold("test", "Rust");
        let md = generate_claude_md(&config);
        let line_count = md.lines().count();
        assert!(
            line_count <= 45,
            "CLAUDE.md is {line_count} lines, should be <= 45"
        );
    }
}

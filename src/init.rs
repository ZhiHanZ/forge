use crate::config::ForgeConfig;
use crate::context::ContextManager;
use crate::skills;
use crate::template;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("context error: {0}")]
    Context(#[from] crate::context::ContextError),
    #[error("feature error: {0}")]
    Feature(#[from] crate::features::FeatureError),
    #[error("project already initialized: forge.toml exists")]
    AlreadyInitialized,
}

/// Initialize a forge project in the given directory.
pub fn init_project(project_dir: &Path, description: &str) -> Result<(), InitError> {
    let config_path = project_dir.join("forge.toml");
    if config_path.exists() {
        return Err(InitError::AlreadyInitialized);
    }

    // Parse name from description (first word or slug)
    let name = slugify_name(description);

    // Create forge.toml
    let config = ForgeConfig::scaffold(&name, "");
    config.save(project_dir)?;

    // Create directories
    let ctx = ContextManager::new(project_dir);
    ctx.init()?;
    std::fs::create_dir_all(project_dir.join("feedback"))?;
    std::fs::create_dir_all(project_dir.join("scripts/verify"))?;

    // Generate CLAUDE.md and AGENTS.md
    let claude_md = template::generate_claude_md(&config);
    std::fs::write(project_dir.join("CLAUDE.md"), &claude_md)?;
    std::fs::write(project_dir.join("AGENTS.md"), &claude_md)?;

    // Create empty features.json
    let features = crate::features::FeatureList {
        features: vec![],
    };
    features.save(project_dir)?;

    // Create references/ dir and add to .gitignore
    std::fs::create_dir_all(project_dir.join("references"))?;
    append_gitignore(project_dir, "references/")?;

    // Install skills
    install_skills(project_dir)?;

    Ok(())
}

/// Install/update an existing forge project: skills, CLAUDE.md, directories, permissions.
pub fn install_project(project_dir: &Path) -> Result<(), InitError> {
    let config = ForgeConfig::load(project_dir)?;

    // Install/update skills
    install_skills(project_dir)?;

    // Regenerate CLAUDE.md and AGENTS.md from current config
    let claude_md = template::generate_claude_md(&config);
    std::fs::write(project_dir.join("CLAUDE.md"), &claude_md)?;
    std::fs::write(project_dir.join("AGENTS.md"), &claude_md)?;

    // Ensure directories exist
    let ctx = ContextManager::new(project_dir);
    ctx.init()?;
    std::fs::create_dir_all(project_dir.join("feedback"))?;
    std::fs::create_dir_all(project_dir.join("scripts/verify"))?;
    std::fs::create_dir_all(project_dir.join(".forge"))?;
    std::fs::create_dir_all(project_dir.join("references"))?;
    append_gitignore(project_dir, "references/")?;

    // Regenerate context INDEX.md
    ctx.write_index()?;

    // chmod +x on verify scripts
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let verify_dir = project_dir.join("scripts/verify");
        if verify_dir.is_dir() {
            for entry in std::fs::read_dir(&verify_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("sh") {
                    let mut perms = std::fs::metadata(&path)?.permissions();
                    perms.set_mode(perms.mode() | 0o111);
                    std::fs::set_permissions(&path, perms)?;
                }
            }
        }
    }

    Ok(())
}

/// Install all forge skills into .claude/skills/.
pub fn install_skills(project_dir: &Path) -> Result<(), std::io::Error> {
    for (skill_name, files) in skills::all_skills() {
        let skill_dir = project_dir.join(".claude/skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir)?;
        for (filename, content) in files {
            std::fs::write(skill_dir.join(filename), content)?;
        }
    }
    Ok(())
}

/// Append an entry to .gitignore if not already present.
fn append_gitignore(project_dir: &Path, entry: &str) -> Result<(), std::io::Error> {
    let gitignore = project_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "{entry}")?;
    Ok(())
}

fn slugify_name(description: &str) -> String {
    description
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "My Test App").unwrap();

        // forge.toml
        assert!(dir.path().join("forge.toml").exists());
        let config = ForgeConfig::load(dir.path()).unwrap();
        assert_eq!(config.project.name, "my-test-app");

        // CLAUDE.md and AGENTS.md
        assert!(dir.path().join("CLAUDE.md").exists());
        assert!(dir.path().join("AGENTS.md").exists());
        let claude = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(claude, agents);
        assert!(claude.contains("# my-test-app"));

        // features.json
        assert!(dir.path().join("features.json").exists());

        // context dirs
        assert!(dir.path().join("context/decisions").is_dir());
        assert!(dir.path().join("context/gotchas").is_dir());
        assert!(dir.path().join("context/patterns").is_dir());
        assert!(dir.path().join("context/poc").is_dir());
        assert!(dir.path().join("context/references").is_dir());

        // feedback and scripts
        assert!(dir.path().join("feedback").is_dir());
        assert!(dir.path().join("scripts/verify").is_dir());
    }

    #[test]
    fn init_installs_skills() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        // All 4 skills installed
        assert!(dir
            .path()
            .join(".claude/skills/forge-planning/SKILL.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-planning/COVERAGE.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-protocol/SKILL.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-protocol/CLAIMING.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-protocol/CONTEXT-WRITING.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-protocol/CONTEXT-READING.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-protocol/TESTING.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-orchestrating/SKILL.md")
            .exists());
        assert!(dir
            .path()
            .join(".claude/skills/forge-adjusting/SKILL.md")
            .exists());

        // Skill files have content
        let planning = std::fs::read_to_string(
            dir.path().join(".claude/skills/forge-planning/SKILL.md"),
        )
        .unwrap();
        assert!(planning.contains("forge-planning"));
    }

    #[test]
    fn init_fails_if_already_initialized() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();
        let result = init_project(dir.path(), "test again");
        assert!(matches!(result, Err(InitError::AlreadyInitialized)));
    }

    #[test]
    fn slugify_name_works() {
        assert_eq!(slugify_name("My Test App"), "my-test-app");
        assert_eq!(slugify_name("REST API with CRUD"), "rest-api-with");
        assert_eq!(slugify_name("simple"), "simple");
        assert_eq!(
            slugify_name("Hello World! 123"),
            "hello-world-123"
        );
    }

    #[test]
    fn features_json_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();
        let features = crate::features::FeatureList::load(dir.path()).unwrap();
        assert!(features.features.is_empty());
    }

    #[test]
    fn install_on_existing_project() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        // Delete skills
        std::fs::remove_dir_all(dir.path().join(".claude/skills")).unwrap();
        assert!(!dir.path().join(".claude/skills/forge-planning/SKILL.md").exists());

        // Install restores them
        install_project(dir.path()).unwrap();
        assert!(dir.path().join(".claude/skills/forge-planning/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/forge-protocol/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/forge-orchestrating/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/forge-adjusting/SKILL.md").exists());
    }

    #[test]
    fn install_regenerates_claude_md() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        // Modify the config name
        let mut config = ForgeConfig::load(dir.path()).unwrap();
        config.project.name = "renamed-project".into();
        config.save(dir.path()).unwrap();

        // Install regenerates CLAUDE.md with updated name
        install_project(dir.path()).unwrap();
        let claude = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(claude.contains("# renamed-project"));
        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("# renamed-project"));
    }

    #[cfg(unix)]
    #[test]
    fn install_fixes_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        // Create a script without +x
        let script = dir.path().join("scripts/verify/check.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let perms = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(&script, perms).unwrap();

        // Install should fix permissions
        install_project(dir.path()).unwrap();
        let mode = std::fs::metadata(&script).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "script should be executable after install");
    }

    #[test]
    fn init_creates_references_dir_and_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        assert!(dir.path().join("references").is_dir());
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("references/"));
    }

    #[test]
    fn install_creates_references_and_index() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();

        // Add a context entry, then install to regenerate index
        let ctx = ContextManager::new(dir.path());
        ctx.write_entry("decisions", "use-vec", "# Use Vec<u8>\nSimpler.").unwrap();
        install_project(dir.path()).unwrap();

        // INDEX.md should exist with the entry
        let index = std::fs::read_to_string(dir.path().join("context/INDEX.md")).unwrap();
        assert!(index.contains("use-vec"));
    }

    #[test]
    fn gitignore_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), "test").unwrap();
        // Install again — should not duplicate
        install_project(dir.path()).unwrap();

        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        let count = gitignore.matches("references/").count();
        assert_eq!(count, 1, "references/ should appear exactly once in .gitignore");
    }

    #[test]
    fn install_fails_without_forge_toml() {
        let dir = tempfile::tempdir().unwrap();
        let result = install_project(dir.path());
        assert!(result.is_err());
    }
}

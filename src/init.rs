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

    // Install skills
    install_skills(project_dir)?;

    Ok(())
}

/// Install all forge skills into .claude/skills/.
fn install_skills(project_dir: &Path) -> Result<(), std::io::Error> {
    for (skill_name, files) in skills::all_skills() {
        let skill_dir = project_dir.join(".claude/skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir)?;
        for (filename, content) in files {
            std::fs::write(skill_dir.join(filename), content)?;
        }
    }
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
}

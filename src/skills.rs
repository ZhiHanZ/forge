/// Embedded skill files. Written to .claude/skills/ and .agents/skills/ by forge init.
///
/// Each skill is a (relative_path, content) pair.

use std::path::Path;

/// Sync all skills to both .claude/skills/ and .agents/skills/.
/// Called at the start of forge run to ensure existing projects have
/// Codex-compatible skills without requiring re-init.
pub fn sync_skills(project_dir: &Path) -> Result<(), std::io::Error> {
    let dirs = [".claude/skills", ".agents/skills"];
    for (skill_name, files) in all_skills() {
        for base in &dirs {
            let skill_dir = project_dir.join(base).join(skill_name);
            std::fs::create_dir_all(&skill_dir)?;
            for (filename, content) in &files {
                std::fs::write(skill_dir.join(filename), content)?;
            }
        }
    }
    Ok(())
}

pub fn forge_planning_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SKILL.md", include_str!("../skills/forge-planning/SKILL.md")),
        (
            "COVERAGE.md",
            include_str!("../skills/forge-planning/COVERAGE.md"),
        ),
        (
            "REFERENCES.md",
            include_str!("../skills/forge-planning/REFERENCES.md"),
        ),
    ]
}

pub fn forge_protocol_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "SKILL.md",
            include_str!("../skills/forge-protocol/SKILL.md"),
        ),
        (
            "CLAIMING.md",
            include_str!("../skills/forge-protocol/CLAIMING.md"),
        ),
        (
            "CONTEXT-WRITING.md",
            include_str!("../skills/forge-protocol/CONTEXT-WRITING.md"),
        ),
        (
            "CONTEXT-READING.md",
            include_str!("../skills/forge-protocol/CONTEXT-READING.md"),
        ),
        (
            "TESTING.md",
            include_str!("../skills/forge-protocol/TESTING.md"),
        ),
    ]
}

pub fn forge_orchestrating_files() -> Vec<(&'static str, &'static str)> {
    vec![(
        "SKILL.md",
        include_str!("../skills/forge-orchestrating/SKILL.md"),
    )]
}

pub fn forge_adjusting_files() -> Vec<(&'static str, &'static str)> {
    vec![(
        "SKILL.md",
        include_str!("../skills/forge-adjusting/SKILL.md"),
    )]
}

/// All skills with their directory names.
pub fn all_skills() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("forge-planning", forge_planning_files()),
        ("forge-protocol", forge_protocol_files()),
        ("forge-orchestrating", forge_orchestrating_files()),
        ("forge-adjusting", forge_adjusting_files()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_skills_have_skill_md() {
        for (name, files) in all_skills() {
            let has_skill_md = files.iter().any(|(path, _)| *path == "SKILL.md");
            assert!(has_skill_md, "Skill {name} missing SKILL.md");
        }
    }

    #[test]
    fn skill_md_has_frontmatter() {
        for (name, files) in all_skills() {
            let (_, content) = files.iter().find(|(p, _)| *p == "SKILL.md").unwrap();
            assert!(
                content.starts_with("---"),
                "Skill {name}/SKILL.md missing YAML frontmatter"
            );
            assert!(
                content.contains("name:"),
                "Skill {name}/SKILL.md missing name field"
            );
            assert!(
                content.contains("description:"),
                "Skill {name}/SKILL.md missing description field"
            );
        }
    }

    #[test]
    fn skill_md_under_500_lines() {
        for (name, files) in all_skills() {
            let (_, content) = files.iter().find(|(p, _)| *p == "SKILL.md").unwrap();
            let lines = content.lines().count();
            assert!(
                lines <= 500,
                "Skill {name}/SKILL.md is {lines} lines, max 500"
            );
        }
    }

    #[test]
    fn planning_has_coverage_and_references() {
        let files = forge_planning_files();
        assert!(files.iter().any(|(p, _)| *p == "COVERAGE.md"));
        assert!(files.iter().any(|(p, _)| *p == "REFERENCES.md"));
    }

    #[test]
    fn protocol_has_claiming_and_context() {
        let files = forge_protocol_files();
        assert!(files.iter().any(|(p, _)| *p == "CLAIMING.md"));
        assert!(files.iter().any(|(p, _)| *p == "CONTEXT-WRITING.md"));
        assert!(files.iter().any(|(p, _)| *p == "CONTEXT-READING.md"));
        assert!(files.iter().any(|(p, _)| *p == "TESTING.md"));
    }

    #[test]
    fn sync_skills_populates_both_dirs() {
        let dir = tempfile::tempdir().unwrap();
        sync_skills(dir.path()).unwrap();

        for base in &[".claude/skills", ".agents/skills"] {
            for (skill_name, files) in all_skills() {
                for (filename, _) in &files {
                    let path = dir.path().join(base).join(skill_name).join(filename);
                    assert!(path.exists(), "missing: {}", path.display());
                }
            }
        }
    }

    #[test]
    fn sync_skills_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        sync_skills(dir.path()).unwrap();
        sync_skills(dir.path()).unwrap(); // second call should not fail

        let path = dir.path().join(".agents/skills/forge-protocol/SKILL.md");
        assert!(path.exists());
    }
}

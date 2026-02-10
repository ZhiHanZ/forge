use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::features::{FeatureList, FeatureStatus};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a forge project (missing forge.toml)")]
    NotInitialized,
}

#[derive(Debug, Serialize)]
pub struct ExportManifest {
    pub forge_version: String,
    pub exported_at: String,
    pub project_dir: String,
    pub project_name: String,
    pub features: FeatureSummary,
    pub context_counts: BTreeMap<String, usize>,
    pub logs: Vec<String>,
    pub transcripts: Vec<TranscriptInfo>,
    pub git: Option<GitInfo>,
    pub sections: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FeatureSummary {
    pub total: usize,
    pub done: usize,
    pub pending: usize,
    pub claimed: usize,
    pub blocked: usize,
}

#[derive(Debug, Serialize)]
pub struct TranscriptInfo {
    pub session_id: String,
    pub size_bytes: u64,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct GitInfo {
    pub commits_included: usize,
    pub branch: String,
    pub latest_commit: String,
}

pub fn export_project(
    project_dir: &Path,
    output_dir: &Path,
    include_transcripts: bool,
    git_commits: usize,
) -> Result<ExportManifest, ExportError> {
    // Verify this is a forge project
    if !project_dir.join("forge.toml").exists() {
        return Err(ExportError::NotInitialized);
    }

    // Clean and create output directory
    if output_dir.exists() {
        std::fs::remove_dir_all(output_dir)?;
    }
    std::fs::create_dir_all(output_dir)?;

    let mut sections = Vec::new();

    // Copy forge.toml
    if copy_if_exists(&project_dir.join("forge.toml"), &output_dir.join("forge.toml")) {
        sections.push("config".to_string());
    }

    // Copy features.json
    copy_if_exists(
        &project_dir.join("features.json"),
        &output_dir.join("features.json"),
    );

    // Copy agent instruction files
    copy_if_exists(&project_dir.join("CLAUDE.md"), &output_dir.join("CLAUDE.md"));
    copy_if_exists(&project_dir.join("AGENTS.md"), &output_dir.join("AGENTS.md"));

    // Copy feedback/
    let feedback_src = project_dir.join("feedback");
    if feedback_src.is_dir() {
        let feedback_dst = output_dir.join("feedback");
        let count = copy_dir_recursive(&feedback_src, &feedback_dst)?;
        if count > 0 {
            sections.push("feedback".to_string());
        }
    }

    // Copy context/
    let context_src = project_dir.join("context");
    if context_src.is_dir() {
        let context_dst = output_dir.join("context");
        copy_dir_recursive(&context_src, &context_dst)?;
        sections.push("context".to_string());
    }

    // Copy skills from .claude/skills/
    let skills_src = project_dir.join(".claude/skills");
    if skills_src.is_dir() {
        let skills_dst = output_dir.join("skills");
        let count = copy_dir_recursive(&skills_src, &skills_dst)?;
        if count > 0 {
            sections.push("skills".to_string());
        }
    }

    // Copy agent logs
    let logs_src = project_dir.join(".forge/logs");
    let mut log_names = Vec::new();
    if logs_src.is_dir() {
        let logs_dst = output_dir.join("logs");
        std::fs::create_dir_all(&logs_dst)?;
        if let Ok(entries) = std::fs::read_dir(&logs_src) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    std::fs::copy(&path, logs_dst.join(&name))?;
                    log_names.push(name);
                }
            }
        }
        if !log_names.is_empty() {
            sections.push("logs".to_string());
        }
    }
    log_names.sort();

    // Git data
    let git_info = capture_git_info(project_dir, output_dir, git_commits)?;
    if git_info.is_some() {
        sections.push("git".to_string());
    }

    // Transcripts
    let mut transcripts = Vec::new();
    if include_transcripts
        && let Some(transcript_dir) = find_transcript_dir(project_dir)
    {
        let transcripts_dst = output_dir.join("transcripts");
        std::fs::create_dir_all(&transcripts_dst)?;

        if let Ok(entries) = std::fs::read_dir(&transcript_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") && path.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    std::fs::copy(&path, transcripts_dst.join(&name))?;

                    let session_id = name.trim_end_matches(".jsonl").to_string();
                    transcripts.push(TranscriptInfo {
                        session_id,
                        size_bytes: size,
                        path: format!("transcripts/{name}"),
                    });
                }
            }
        }
        if !transcripts.is_empty() {
            sections.push("transcripts".to_string());
        }
    }
    transcripts.sort_by(|a, b| a.session_id.cmp(&b.session_id));

    // Build feature summary
    let feature_summary = build_feature_summary(project_dir);

    // Build context counts
    let context_counts = count_context_entries(project_dir);

    // Project name from directory
    let project_name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let manifest = ExportManifest {
        forge_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        project_dir: project_dir
            .canonicalize()
            .unwrap_or_else(|_| project_dir.to_path_buf())
            .to_string_lossy()
            .to_string(),
        project_name,
        features: feature_summary,
        context_counts,
        logs: log_names,
        transcripts,
        git: git_info,
        sections,
    };

    // Write manifest
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(output_dir.join("manifest.json"), manifest_json)?;

    Ok(manifest)
}

fn copy_if_exists(src: &Path, dst: &Path) -> bool {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::copy(src, dst).is_ok()
    } else {
        false
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<usize, std::io::Error> {
    let mut count = 0;
    if !src.is_dir() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)?.flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            count += copy_dir_recursive(&path, &dest)?;
        } else if path.is_file() {
            std::fs::copy(&path, &dest)?;
            count += 1;
        }
    }
    Ok(count)
}

fn find_transcript_dir(project_dir: &Path) -> Option<PathBuf> {
    let canonical = project_dir.canonicalize().ok()?;
    let dir_name = canonical.to_string_lossy().replace('/', "-");
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home)
        .join(".claude/projects")
        .join(&dir_name);
    if path.is_dir() { Some(path) } else { None }
}

fn capture_git_info(
    project_dir: &Path,
    output_dir: &Path,
    commits: usize,
) -> Result<Option<GitInfo>, ExportError> {
    // Check if this is a git repo
    let status = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(project_dir)
        .output();

    let Ok(output) = status else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }

    let git_dst = output_dir.join("git");
    std::fs::create_dir_all(&git_dst)?;

    // git log
    let log_output = Command::new("git")
        .args([
            "log",
            "--format=%H %aI %an %s",
            &format!("-{commits}"),
        ])
        .current_dir(project_dir)
        .output()?;
    let log_text = String::from_utf8_lossy(&log_output.stdout).to_string();
    std::fs::write(git_dst.join("log.txt"), &log_text)?;

    // git diff --stat
    let diff_output = Command::new("git")
        .args([
            "diff",
            "--stat",
            &format!("HEAD~{commits}..HEAD"),
        ])
        .current_dir(project_dir)
        .output()?;
    let diff_text = String::from_utf8_lossy(&diff_output.stdout).to_string();
    if !diff_text.is_empty() {
        std::fs::write(git_dst.join("diff-stat.txt"), &diff_text)?;
    }

    // Extract info for manifest
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project_dir)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let latest_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(project_dir)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let commits_included = log_text.lines().count();

    Ok(Some(GitInfo {
        commits_included,
        branch,
        latest_commit,
    }))
}

fn build_feature_summary(project_dir: &Path) -> FeatureSummary {
    let list = FeatureList::load(project_dir).ok();
    match list {
        Some(fl) => {
            let total = fl.features.len();
            let done = fl
                .features
                .iter()
                .filter(|f| f.status == FeatureStatus::Done)
                .count();
            let pending = fl
                .features
                .iter()
                .filter(|f| f.status == FeatureStatus::Pending)
                .count();
            let claimed = fl
                .features
                .iter()
                .filter(|f| f.status == FeatureStatus::Claimed)
                .count();
            let blocked = fl
                .features
                .iter()
                .filter(|f| f.status == FeatureStatus::Blocked)
                .count();
            FeatureSummary {
                total,
                done,
                pending,
                claimed,
                blocked,
            }
        }
        None => FeatureSummary {
            total: 0,
            done: 0,
            pending: 0,
            claimed: 0,
            blocked: 0,
        },
    }
}

fn count_context_entries(project_dir: &Path) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let context_dir = project_dir.join("context");
    if !context_dir.is_dir() {
        return counts;
    }

    for entry in std::fs::read_dir(&context_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            let category = entry.file_name().to_string_lossy().to_string();
            let count = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_file())
                .count();
            if count > 0 {
                counts.insert(category, count);
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_project(dir: &Path) {
        fs::write(dir.join("forge.toml"), "[project]\nname = \"test\"\n").unwrap();
        fs::write(
            dir.join("features.json"),
            r#"{"features":[{"id":"f1","type":"implement","scope":"test","description":"Test feature","verify":"true","status":"done"}]}"#,
        )
        .unwrap();
        fs::write(dir.join("CLAUDE.md"), "# Agent Instructions\n").unwrap();

        // context/
        let ctx = dir.join("context/decisions");
        fs::create_dir_all(&ctx).unwrap();
        fs::write(ctx.join("arch.md"), "Architecture decision").unwrap();
        fs::write(
            dir.join("context/INDEX.md"),
            "# Context Index\n- decisions/arch\n",
        )
        .unwrap();

        // feedback/
        let fb = dir.join("feedback");
        fs::create_dir_all(&fb).unwrap();
        fs::write(fb.join("last-verify.json"), r#"{"pass":1,"fail":0}"#).unwrap();

        // logs/
        let logs = dir.join(".forge/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("agent-1.log"), "some log output\n").unwrap();
    }

    #[test]
    fn test_export_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let err = export_project(tmp.path(), &out, false, 10).unwrap_err();
        assert!(matches!(err, ExportError::NotInitialized));
    }

    #[test]
    fn test_export_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        setup_test_project(&project);

        let out = tmp.path().join("export");
        let manifest = export_project(&project, &out, false, 10).unwrap();

        // Check manifest
        assert_eq!(manifest.features.total, 1);
        assert_eq!(manifest.features.done, 1);
        assert!(manifest.sections.contains(&"config".to_string()));
        assert!(manifest.sections.contains(&"feedback".to_string()));
        assert!(manifest.sections.contains(&"context".to_string()));
        assert!(manifest.sections.contains(&"logs".to_string()));

        // Check files exist
        assert!(out.join("forge.toml").exists());
        assert!(out.join("features.json").exists());
        assert!(out.join("CLAUDE.md").exists());
        assert!(out.join("manifest.json").exists());
        assert!(out.join("feedback/last-verify.json").exists());
        assert!(out.join("context/INDEX.md").exists());
        assert!(out.join("context/decisions/arch.md").exists());
        assert!(out.join("logs/agent-1.log").exists());

        // Context counts
        assert_eq!(manifest.context_counts.get("decisions"), Some(&1));

        // Logs list
        assert_eq!(manifest.logs, vec!["agent-1.log"]);
    }

    #[test]
    fn test_export_overwrites_previous() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        setup_test_project(&project);

        let out = tmp.path().join("export");

        // First export
        export_project(&project, &out, false, 10).unwrap();
        // Place a stale file
        fs::write(out.join("stale.txt"), "old").unwrap();

        // Second export should remove stale file
        export_project(&project, &out, false, 10).unwrap();
        assert!(!out.join("stale.txt").exists());
    }

    #[test]
    fn test_copy_if_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.txt");
        let dst = tmp.path().join("dst.txt");

        // File doesn't exist
        assert!(!copy_if_exists(&src, &dst));

        // File exists
        fs::write(&src, "hello").unwrap();
        assert!(copy_if_exists(&src, &dst));
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn test_copy_dir_recursive() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("sub/b.txt"), "b").unwrap();

        let count = copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(count, 2);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "b");
    }

    #[test]
    fn test_feature_summary_no_features() {
        let tmp = tempfile::tempdir().unwrap();
        let summary = build_feature_summary(tmp.path());
        assert_eq!(summary.total, 0);
    }
}

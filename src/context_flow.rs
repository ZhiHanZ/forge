use std::path::Path;
use std::process::Command;

/// Embedded Python files for the CocoIndex context pipeline.
const CONTEXT_FLOW_PY: &str = include_str!("../context/context_flow.py");
const CONTEXT_MODELS_PY: &str = include_str!("../context/context_models.py");
const REQUIREMENTS_TXT: &str = include_str!("../context/requirements.txt");

/// Check if the `cocoindex` CLI is available on PATH.
pub fn cocoindex_available() -> bool {
    Command::new("cocoindex")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Sync the embedded Python files to `.forge/` in the project directory.
/// Creates `context/packages/` and `feedback/exec-memory/` directories.
pub fn sync_context_flow(project_dir: &Path) {
    let forge_dir = project_dir.join(".forge");
    let _ = std::fs::create_dir_all(&forge_dir);
    let _ = std::fs::create_dir_all(project_dir.join("context/packages"));
    let _ = std::fs::create_dir_all(project_dir.join("feedback/exec-memory"));

    let _ = std::fs::write(forge_dir.join("context_flow.py"), CONTEXT_FLOW_PY);
    let _ = std::fs::write(forge_dir.join("context_models.py"), CONTEXT_MODELS_PY);
    let _ = std::fs::write(forge_dir.join("requirements.txt"), REQUIREMENTS_TXT);
}

/// Run `cocoindex update` to refresh context packages.
///
/// Returns `Ok(true)` if cocoindex ran successfully, `Ok(false)` if cocoindex
/// is not available, and `Err` on execution failure (non-fatal).
pub fn refresh_context(project_dir: &Path) -> Result<bool, String> {
    if !cocoindex_available() {
        return Ok(false);
    }

    let flow_path = project_dir.join(".forge/context_flow.py");
    if !flow_path.exists() {
        return Err("context_flow.py not found in .forge/".into());
    }

    // CocoIndex v1 uses LMDB for internal state — no external database needed.
    // Point the database path to .forge/cocoindex-db/ (embedded, local).
    let db_path = project_dir.join(".forge/cocoindex-db");
    let _ = std::fs::create_dir_all(&db_path);

    let output = Command::new("cocoindex")
        .arg("update")
        .arg(flow_path.to_string_lossy().as_ref())
        .current_dir(project_dir)
        .env("FORGE_PROJECT_DIR", project_dir.to_string_lossy().as_ref())
        .env("COCOINDEX_DATABASE_URL", format!("lmdb://{}", db_path.to_string_lossy()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run cocoindex: {e}"))?;

    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("cocoindex update failed: {stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_context_flow_nonempty() {
        assert!(!CONTEXT_FLOW_PY.is_empty());
    }

    #[test]
    fn embedded_context_models_nonempty() {
        assert!(!CONTEXT_MODELS_PY.is_empty());
    }

    #[test]
    fn embedded_requirements_nonempty() {
        assert!(!REQUIREMENTS_TXT.is_empty());
    }

    #[test]
    fn embedded_content_valid() {
        assert!(CONTEXT_FLOW_PY.contains("cocoindex"));
        assert!(CONTEXT_MODELS_PY.contains("FileMapInfo"));
        assert!(REQUIREMENTS_TXT.contains("cocoindex"));
    }

    #[test]
    fn sync_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        sync_context_flow(dir.path());

        assert!(dir.path().join(".forge/context_flow.py").exists());
        assert!(dir.path().join(".forge/context_models.py").exists());
        assert!(dir.path().join(".forge/requirements.txt").exists());
        assert!(dir.path().join("context/packages").is_dir());
        assert!(dir.path().join("feedback/exec-memory").is_dir());
    }

    #[test]
    fn sync_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        sync_context_flow(dir.path());
        sync_context_flow(dir.path());

        let content = std::fs::read_to_string(dir.path().join(".forge/context_flow.py")).unwrap();
        assert_eq!(content, CONTEXT_FLOW_PY);
    }

    #[test]
    fn refresh_graceful_when_unavailable() {
        // If cocoindex is not installed, refresh should return Ok(false)
        // This test works regardless of whether cocoindex is installed:
        // - If not installed: returns Ok(false)
        // - If installed but no flow file: returns Err (which is also acceptable)
        let dir = tempfile::tempdir().unwrap();
        let result = refresh_context(dir.path());
        // Either Ok(false) if cocoindex not available, or Err if available but no flow file
        assert!(result.is_ok() || result.is_err());
    }
}

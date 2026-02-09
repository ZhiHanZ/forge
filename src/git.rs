use std::path::Path;
use std::process::Command;

/// Check if directory is inside a git work tree.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if the repo has a remote configured.
pub fn has_remote(dir: &Path) -> bool {
    Command::new("git")
        .args(["remote"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Pull with rebase + autostash. No-op if no remote.
pub fn pull(dir: &Path) -> Result<(), String> {
    if !has_remote(dir) {
        return Ok(());
    }
    let output = Command::new("git")
        .args(["pull", "--rebase", "--autostash"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git pull failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git pull failed: {stderr}"));
    }
    Ok(())
}

/// Stage all changes and commit. Returns false if nothing to commit.
pub fn add_and_commit(dir: &Path, message: &str) -> Result<bool, String> {
    // Stage all
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git add failed: {e}"))?;

    // Check for staged changes
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(dir)
        .status()
        .map_err(|e| format!("git diff failed: {e}"))?;

    if status.success() {
        return Ok(false); // nothing staged
    }

    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git commit failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git commit failed: {stderr}"));
    }
    Ok(true)
}

/// Push to remote. No-op if no remote. Returns false if push fails (e.g. conflict).
pub fn push(dir: &Path) -> Result<bool, String> {
    if !has_remote(dir) {
        return Ok(true);
    }
    let output = Command::new("git")
        .args(["push"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git push failed: {e}"))?;
    Ok(output.status.success())
}

/// Create a git worktree for an agent.
pub fn create_worktree(repo_dir: &Path, worktree_dir: &Path, branch: &str) -> Result<(), String> {
    // Create branch if it doesn't exist
    let _ = Command::new("git")
        .args(["branch", branch])
        .current_dir(repo_dir)
        .output();

    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            &worktree_dir.to_string_lossy(),
            branch,
        ])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("git worktree add failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {stderr}"));
    }
    Ok(())
}

/// Remove a git worktree.
pub fn remove_worktree(repo_dir: &Path, worktree_dir: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            &worktree_dir.to_string_lossy(),
        ])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("git worktree remove failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree remove failed: {stderr}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        // Initial commit so HEAD exists
        std::fs::write(dir.join("README.md"), "# test\n").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn is_git_repo_detects_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(dir.path()));
        init_repo(dir.path());
        assert!(is_git_repo(dir.path()));
    }

    #[test]
    fn has_remote_false_for_local() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        assert!(!has_remote(dir.path()));
    }

    #[test]
    fn add_and_commit_works() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        // Nothing to commit
        assert!(!add_and_commit(dir.path(), "empty").unwrap());

        // Create file and commit
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        assert!(add_and_commit(dir.path(), "add test").unwrap());

        // Nothing to commit again
        assert!(!add_and_commit(dir.path(), "empty again").unwrap());
    }

    #[test]
    fn push_noop_without_remote() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        assert!(push(dir.path()).unwrap());
    }

    #[test]
    fn pull_noop_without_remote() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        pull(dir.path()).unwrap();
    }

    #[test]
    fn worktree_create_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let wt = dir.path().join("worktree-agent-1");
        create_worktree(dir.path(), &wt, "agent-1").unwrap();
        assert!(wt.exists());
        assert!(is_git_repo(&wt));

        remove_worktree(dir.path(), &wt).unwrap();
        assert!(!wt.exists());
    }
}

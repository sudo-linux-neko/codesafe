//! Git helpers used by the safe-fix loop.

use crate::Result;
use std::path::Path;

pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").is_dir()
}

pub fn is_dirty(path: &Path) -> Result<bool> {
    if !is_git_repo(path) {
        return Ok(false);
    }
    let output = std::process::Command::new("git")
        .args(["-C", path.to_str().unwrap_or("."), "status", "--porcelain"])
        .output()
        .map_err(|e| crate::Error::Git(format!("git status: {e}")))?;
    Ok(!output.stdout.is_empty())
}

pub fn checkpoint(path: &Path) -> Result<String> {
    let head = current_head(path)?;
    commit(
        path,
        &format!("codeguard: checkpoint before safe-fix loop ({head})"),
    )?;
    Ok(head)
}

pub fn commit(path: &Path, message: &str) -> Result<()> {
    let status = std::process::Command::new("git")
        .args(["-C", path.to_str().unwrap_or("."), "add", "-A"])
        .status()
        .map_err(|e| crate::Error::Git(format!("git add: {e}")))?;
    if !status.success() {
        return Err(crate::Error::Git("git add failed".into()));
    }
    // Only commit if there is something staged.
    let diff = std::process::Command::new("git")
        .args([
            "-C",
            path.to_str().unwrap_or("."),
            "diff",
            "--cached",
            "--quiet",
        ])
        .status()
        .map_err(|e| crate::Error::Git(format!("git diff: {e}")))?;
    if diff.success() {
        return Ok(()); // nothing to commit
    }
    let status = std::process::Command::new("git")
        .args(["-C", path.to_str().unwrap_or("."), "commit", "-m", message])
        .status()
        .map_err(|e| crate::Error::Git(format!("git commit: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::Error::Git("git commit failed".into()))
    }
}

pub fn current_head(path: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            path.to_str().unwrap_or("."),
            "rev-parse",
            "--short",
            "HEAD",
        ])
        .output()
        .map_err(|e| crate::Error::Git(format!("git rev-parse: {e}")))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn revert_to(path: &Path, reference: &str) -> Result<()> {
    let status = std::process::Command::new("git")
        .args([
            "-C",
            path.to_str().unwrap_or("."),
            "reset",
            "--hard",
            reference,
        ])
        .status()
        .map_err(|e| crate::Error::Git(format!("git reset: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::Error::Git("git reset failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_git_dir_is_not_repo() {
        let dir = std::env::temp_dir().join(format!("codeguard-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_git_repo(&dir));
        assert!(!is_dirty(&dir).unwrap());
    }
}

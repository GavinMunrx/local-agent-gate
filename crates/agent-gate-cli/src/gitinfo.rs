use std::path::PathBuf;
use std::process::Command;

pub struct GitInfo {
    pub project_path: PathBuf,
    pub remote: Option<String>,
    pub branch: Option<String>,
}

pub fn discover(cwd: &std::path::Path) -> GitInfo {
    let toplevel = run_git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());
    let remote = run_git(cwd, &["config", "--get", "remote.origin.url"]);
    let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);

    GitInfo {
        project_path: toplevel,
        remote,
        branch,
    }
}

fn run_git(cwd: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

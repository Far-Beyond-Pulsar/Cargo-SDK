//! Moving an SDK in and out of a GitHub release: a `.tar.gz` made and unpacked by the system `tar`
//! (present on every GitHub runner and on Windows 10+), downloaded with `gh` or, without it, `curl`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    status.success().then_some(()).ok_or_else(|| format!("{what} failed ({status})"))
}

/// Packs the SDK directory `dir` into `archive` (`.tar.gz`).
pub fn create(dir: &Path, archive: &Path) -> Result<(), String> {
    run(Command::new("tar").arg("-czf").arg(archive).arg("-C").arg(dir).arg("."), "tar")
}

/// Unpacks `archive` into `dir`.
pub fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    run(Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(dir), "tar")
}

fn has(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Downloads the release asset matching `pattern` (an exact file name) of `repo` at `tag` (`latest` or a tag)
/// into `dir`, and returns its path.
pub fn download(repo: &str, tag: &str, pattern: &str, dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if has("gh") {
        let mut gh = Command::new("gh");
        gh.args(["release", "download"]);
        if tag != "latest" {
            gh.arg(tag);
        }
        gh.args(["--repo", repo, "--pattern", pattern, "--dir"]).arg(dir).arg("--clobber");
        run(&mut gh, "gh release download")?;
    } else {
        // No gh: the public API names the asset's URL.
        let api = if tag == "latest" {
            format!("https://api.github.com/repos/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
        };
        let output = Command::new("curl").args(["-fsSL", &api]).output().map_err(|e| format!("curl: {e}"))?;
        let release: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("{api}: {e}"))?;
        let url = release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|asset| asset["name"] == pattern)
            .and_then(|asset| asset["browser_download_url"].as_str())
            .ok_or_else(|| format!("{repo} {tag} has no asset named {pattern}"))?
            .to_owned();
        run(Command::new("curl").args(["-fsSL", "-o"]).arg(dir.join(pattern)).arg(&url), "curl")?;
    }
    let path = dir.join(pattern);
    path.is_file().then_some(path).ok_or_else(|| format!("{pattern} was not downloaded"))
}

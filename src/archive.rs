//! Moving an SDK in and out of a GitHub release: a `.tar.gz` made and unpacked by the system `tar`
//! (present on every GitHub runner and on Windows 10+), downloaded with `gh` or, without it, `curl`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{what} failed ({status})"))
}

/// Packs the SDK directory `dir` into `archive` (`.tar.gz`).
pub fn create(dir: &Path, archive: &Path) -> Result<(), String> {
    let (parent, name) = split(archive)?;
    let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    run(
        Command::new("tar")
            .current_dir(parent)
            .arg("-czf")
            .arg(name)
            .arg("-C")
            .arg(dir)
            .arg("."),
        "tar",
    )
}

/// Unpacks `archive` into `dir`.
pub fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (parent, name) = split(archive)?;
    let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    run(
        Command::new("tar")
            .current_dir(parent)
            .arg("-xzf")
            .arg(name)
            .arg("-C")
            .arg(dir),
        "tar",
    )
}

/// The directory of `archive` and its file name. GNU tar (the one on `PATH` in Git Bash on Windows) reads
/// `D:\x\a.tar.gz` as the file `\x\a.tar.gz` on a host called `D`, so it is only ever given a bare file name, from
/// the directory it is in.
fn split(archive: &Path) -> Result<(PathBuf, PathBuf), String> {
    let absolute =
        std::path::absolute(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let name = absolute
        .file_name()
        .ok_or_else(|| format!("{}: no file name", archive.display()))?
        .into();
    let parent = absolute.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok((parent, name))
}

fn has(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
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
        gh.args(["--repo", repo, "--pattern", pattern, "--dir"])
            .arg(dir)
            .arg("--clobber");
        run(&mut gh, "gh release download")?;
    } else {
        // No gh: the public API names the asset's URL.
        let api = if tag == "latest" {
            format!("https://api.github.com/repos/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
        };
        let output = Command::new("curl")
            .args(["-fsSL", &api])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        let release: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| format!("{api}: {e}"))?;
        let url = release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|asset| asset["name"] == pattern)
            .and_then(|asset| asset["browser_download_url"].as_str())
            .ok_or_else(|| format!("{repo} {tag} has no asset named {pattern}"))?
            .to_owned();
        run(
            Command::new("curl")
                .args(["-fsSL", "-o"])
                .arg(dir.join(pattern))
                .arg(&url),
            "curl",
        )?;
    }
    let path = dir.join(pattern);
    path.is_file()
        .then_some(path)
        .ok_or_else(|| format!("{pattern} was not downloaded"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_sdk_directory_survives_a_trip_through_an_archive() {
        // Absolute paths with a drive letter on Windows: what GNU tar mistakes for a remote host.
        let root = std::env::temp_dir().join(format!("cargo-sdk-archive-{}", std::process::id()));
        let (from, to, archive) = (root.join("from"), root.join("to"), root.join("sdk.tar.gz"));
        fs::create_dir_all(from.join("units")).unwrap();
        fs::write(from.join("manifest.json"), b"{}").unwrap();
        fs::write(from.join("units").join("libx.rlib"), b"rlib bytes").unwrap();

        create(&from, &archive).unwrap();
        extract(&archive, &to).unwrap();

        assert_eq!(fs::read(to.join("manifest.json")).unwrap(), b"{}");
        assert_eq!(
            fs::read(to.join("units").join("libx.rlib")).unwrap(),
            b"rlib bytes"
        );
        let _ = fs::remove_dir_all(&root);
    }
}

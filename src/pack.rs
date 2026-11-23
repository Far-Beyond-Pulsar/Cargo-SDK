//! `cargo sdk pack`: cut an SDK from a finished build.
//!
//! The SDK holds, for every crate inside the chosen *root* packages (usually the shared libraries) and
//! everything they depend on, what a later build needs in place of compiling it: a stub rlib for a library
//! crate, and the shared library files themselves for a dylib. Which crates those are comes from Cargo's own
//! dependency graph (`cargo metadata`), and what was built from its artifact messages
//! (`cargo build --message-format=json`), so nothing here guesses at Cargo's layout.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::manifest::{Copy, Manifest, Stub, Unit, FORMAT};
use crate::rlib;

pub struct Options {
    /// Packages whose dependency closure the SDK covers, by name.
    pub roots: Vec<String>,
    /// What follows `cargo build`: the build to cut the SDK from (profile, target, features, `-p`).
    pub cargo_args: Vec<String>,
    pub out: PathBuf,
    /// Store metadata-only stubs instead of whole rlibs (smaller; wrong for crates a build script or
    /// proc-macro links, which need the code).
    pub strip: bool,
}

#[derive(Default, Debug)]
pub struct Summary {
    pub library_crates: usize,
    pub shared_libraries: usize,
    pub proc_macros: usize,
    pub skipped: Vec<String>,
    pub original_bytes: u64,
    pub stored_bytes: u64,
}

/// The dependency graph the SDK is cut from.
struct Graph {
    /// Package ids in the closure of the roots (normal and build dependencies).
    closure: HashSet<String>,
    /// Each package's direct normal dependencies: the crates it is compiled against (`--extern`).
    externs: HashMap<String, Vec<String>>,
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn rustc_version() -> Result<String, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .map_err(|e| format!("rustc -vV: {e}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The feature flags of a build, which decide which optional dependencies are in the graph.
fn feature_args(cargo_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = cargo_args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--features" | "-F" => {
                out.push(arg.clone());
                out.extend(args.next().cloned());
            }
            "--all-features" | "--no-default-features" => out.push(arg.clone()),
            other if other.starts_with("--features=") => out.push(arg.clone()),
            _ => {}
        }
    }
    out
}

fn graph(roots: &[String], cargo_args: &[String]) -> Result<Graph, String> {
    let output = cargo()
        .args(["metadata", "--format-version", "1"])
        .args(feature_args(cargo_args))
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        if let (Some(name), Some(id)) = (package["name"].as_str(), package["id"].as_str()) {
            by_name.entry(name).or_default().push(id);
        }
    }
    let mut reach: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut externs: HashMap<String, Vec<String>> = HashMap::new();
    for node in metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(id) = node["id"].as_str() else {
            continue;
        };
        let entry = reach.entry(id).or_default();
        for dep in node["deps"].as_array().into_iter().flatten() {
            let Some(pkg) = dep["pkg"].as_str() else {
                continue;
            };
            let kinds: Vec<&Value> = dep["dep_kinds"].as_array().into_iter().flatten().collect();
            if kinds
                .iter()
                .any(|kind| matches!(kind["kind"].as_str(), None | Some("build")))
            {
                entry.push(pkg);
            }
            if kinds.iter().any(|kind| kind["kind"].is_null()) {
                externs
                    .entry(id.to_owned())
                    .or_default()
                    .push(pkg.to_owned());
            }
        }
    }
    let mut closure: HashSet<String> = HashSet::new();
    let mut stack: Vec<&str> = Vec::new();
    for root in roots {
        stack.extend(
            by_name
                .get(root.as_str())
                .ok_or_else(|| format!("no package named `{root}`"))?,
        );
    }
    while let Some(id) = stack.pop() {
        if closure.insert(id.to_owned()) {
            stack.extend(reach.get(id).into_iter().flatten());
        }
    }
    Ok(Graph { closure, externs })
}

/// `-<hash>` of `lib<crate_name>-<hash>.rlib`.
fn extra_of(crate_name: &str, rlib_file: &str) -> Option<String> {
    let rest = rlib_file.strip_prefix("lib")?.strip_prefix(crate_name)?;
    let extra = rest.strip_suffix(".rlib")?;
    extra.starts_with('-').then(|| extra.to_owned())
}

/// `-<hash>` of a proc-macro's `<name>-<hash>.dll` (`lib<name>-<hash>.so`, `.dylib`).
fn extra_of_library(crate_name: &str, file: &str) -> Option<String> {
    let stem = file.rsplit_once('.')?.0;
    let stem = stem.strip_prefix("lib").unwrap_or(stem);
    let extra = stem.strip_prefix(crate_name)?;
    (extra.starts_with('-')
        && extra.len() == 17
        && extra[1..].bytes().all(|b| b.is_ascii_hexdigit()))
    .then(|| extra.to_owned())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One `compiler-artifact` message, as far as packing cares.
struct Artifact {
    package_id: String,
    crate_name: String,
    crate_types: Vec<String>,
    custom_build: bool,
    files: Vec<PathBuf>,
}

impl Artifact {
    fn parse(message: &Value) -> Self {
        let strings = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        };
        Self {
            package_id: message["package_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            crate_name: message["target"]["name"]
                .as_str()
                .unwrap_or_default()
                .replace('-', "_"),
            crate_types: strings(&message["target"]["crate_types"]),
            custom_build: strings(&message["target"]["kind"])
                .iter()
                .any(|k| k == "custom-build"),
            files: strings(&message["filenames"])
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        }
    }

    fn has(&self, crate_type: &str) -> bool {
        self.crate_types.iter().any(|t| t == crate_type)
    }
}

pub fn pack(options: &Options) -> Result<Summary, String> {
    let graph = graph(&options.roots, &options.cargo_args)?;
    fs::create_dir_all(options.out.join("units")).map_err(|e| e.to_string())?;

    let mut child = cargo()
        .arg("build")
        .args(&options.cargo_args)
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cargo build: {e}"))?;

    let mut summary = Summary::default();
    let mut units: HashMap<(String, String), Unit> = HashMap::new();
    // How each package of the closure is named to rustc by a dependent: see `ident_of_path`.
    let mut idents: HashMap<String, String> = HashMap::new();
    for line in BufReader::new(child.stdout.take().ok_or("no cargo output")?).lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let artifact = Artifact::parse(&message);
        if !graph.closure.contains(&artifact.package_id) {
            continue;
        }
        let crate_name = artifact.crate_name.clone();
        if let Some(ident) = artifact
            .files
            .iter()
            .find_map(|f| crate::ident_of_path(&f.to_string_lossy()))
        {
            idents.insert(artifact.package_id.clone(), ident);
        }

        if artifact.has("proc-macro") && !artifact.custom_build {
            // A proc-macro runs on the build machine and is loaded by crates compiled against it: one compiled
            // anew has other metadata (paths of build-script output are part of it) than the one the stored
            // rlibs were built against, and rustc then refuses those rlibs. So it is stored as built.
            let mut copy = Vec::new();
            let mut extra = None;
            for file in &artifact.files {
                let name = file_name(file);
                if extra.is_none() {
                    extra = extra_of_library(&crate_name, &name);
                }
                let stored = format!("units/{name}");
                let size = fs::copy(file, options.out.join(&stored))
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                summary.original_bytes += size;
                summary.stored_bytes += size;
                copy.push(Copy { stored, name });
            }
            let Some(extra) = extra else {
                summary.skipped.push(format!(
                    "{crate_name} (proc-macro with unrecognised file names)"
                ));
                continue;
            };
            summary.proc_macros += 1;
            units.insert(
                (crate_name.clone(), extra.clone()),
                Unit {
                    crate_name,
                    extra,
                    crate_types: artifact.crate_types,
                    externs: Vec::new(),
                    stub: None,
                    copy,
                },
            );
            continue;
        }
        if artifact.has("bin") || artifact.custom_build {
            summary
                .skipped
                .push(format!("{crate_name} ({})", artifact.crate_types.join(",")));
            continue;
        }
        if artifact.has("dylib") || artifact.has("cdylib") {
            let mut copy = Vec::new();
            for file in &artifact.files {
                let name = file_name(file);
                let stored = format!("units/{name}");
                let size = fs::copy(file, options.out.join(&stored))
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                summary.original_bytes += size;
                summary.stored_bytes += size;
                copy.push(Copy { stored, name });
            }
            // A shared library has no hash in its file names: it is told apart by what it was compiled against.
            let mut externs: Vec<String> = graph
                .externs
                .get(&artifact.package_id)
                .into_iter()
                .flatten()
                .filter_map(|dep| idents.get(dep).cloned())
                .collect();
            externs.sort();
            summary.shared_libraries += 1;
            units.insert(
                (crate_name.clone(), String::new()),
                Unit {
                    crate_name,
                    extra: String::new(),
                    crate_types: artifact.crate_types,
                    externs,
                    stub: None,
                    copy,
                },
            );
            continue;
        }
        let (Some(rlib_path), Some(rmeta_path)) = (
            artifact
                .files
                .iter()
                .find(|f| f.extension().is_some_and(|e| e == "rlib")),
            artifact
                .files
                .iter()
                .find(|f| f.extension().is_some_and(|e| e == "rmeta")),
        ) else {
            summary.skipped.push(format!(
                "{crate_name} (no rlib and rmeta in {:?})",
                artifact.files
            ));
            continue;
        };
        let Some(extra) = extra_of(&crate_name, &file_name(rlib_path)) else {
            summary.skipped.push(format!(
                "{crate_name} (unrecognised file name {})",
                file_name(rlib_path)
            ));
            continue;
        };
        let rlib_bytes =
            fs::read(rlib_path).map_err(|e| format!("{}: {e}", rlib_path.display()))?;
        let rmeta = fs::read(rmeta_path).map_err(|e| format!("{}: {e}", rmeta_path.display()))?;
        let stored = format!("units/{}", file_name(rlib_path));
        let (stub, offset, rmeta_stored) = if options.strip {
            // Metadata only: right for a consumer that links the library's dylib and runs no build script or
            // proc-macro against these crates (those need the machine code).
            let stub =
                rlib::strip(&rlib_bytes).map_err(|e| format!("{}: {e}", rlib_path.display()))?;
            let offset = rlib::find(&stub, &rmeta).ok_or_else(|| {
                format!(
                    "{}: its metadata is not inside its rlib",
                    rlib_path.display()
                )
            })?;
            (stub, offset as u64, None)
        } else {
            // The whole rlib, and the `.rmeta` beside it: the same file serves every use of the unit.
            let rmeta_stored = format!("units/{}", file_name(rmeta_path));
            fs::write(options.out.join(&rmeta_stored), &rmeta).map_err(|e| e.to_string())?;
            (rlib_bytes.clone(), 0, Some(rmeta_stored))
        };
        fs::write(options.out.join(&stored), &stub).map_err(|e| e.to_string())?;
        summary.library_crates += 1;
        summary.original_bytes += rlib_bytes.len() as u64;
        summary.stored_bytes +=
            stub.len() as u64 + if options.strip { 0 } else { rmeta.len() as u64 };
        units.insert(
            (crate_name.clone(), extra.clone()),
            Unit {
                crate_name,
                extra,
                crate_types: artifact.crate_types,
                externs: Vec::new(),
                stub: Some(Stub {
                    stored,
                    rmeta_offset: offset,
                    rmeta_len: rmeta.len() as u64,
                    rmeta_stored,
                }),
                copy: Vec::new(),
            },
        );
    }
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("cargo build failed".into());
    }
    let mut units: Vec<Unit> = units.into_values().collect();
    units.sort_by(|a, b| (&a.crate_name, &a.extra).cmp(&(&b.crate_name, &b.extra)));
    Manifest {
        format: FORMAT,
        rustc: rustc_version()?,
        units,
    }
    .write(&options.out)?;
    Ok(summary)
}

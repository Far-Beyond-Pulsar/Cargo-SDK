//! `sdk-rustc`: the `RUSTC_WRAPPER`.
//!
//! Cargo runs `<wrapper> <rustc> <rustc arguments>` for every crate it compiles. The wrapper answers in
//! rustc's place when the SDK has the crate (same name, same `-C extra-filename` hash, so the same source,
//! features, flags, dependencies and compiler) and otherwise runs the real rustc untouched. Answering means
//! writing the files rustc would have written and printing the artifact messages Cargo reads from it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{Index, Stub, Unit};
use crate::{SDK_ENV, VERBOSE_ENV};

/// What rustc was asked to do, as far as the wrapper cares.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub crate_name: String,
    pub extra: String,
    pub crate_types: Vec<String>,
    pub out_dir: Option<PathBuf>,
    pub emit: Vec<String>,
    /// Whether Cargo asked for JSON artifact notifications (`--json=...artifacts...`).
    pub json_artifacts: bool,
    /// The paths Cargo passed with `--extern name=path`.
    pub externs: Vec<String>,
}

/// The arguments with `@file` response files expanded, as rustc does.
fn expand(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg
            .strip_prefix('@')
            .and_then(|path| fs::read_to_string(path).ok())
        {
            Some(text) => out.extend(text.lines().map(str::to_owned)),
            None => out.push(arg.clone()),
        }
    }
    out
}

/// Reads the arguments Cargo passes to rustc for a compile. `None` for anything else (`-vV`, `--print`).
pub fn parse(args: &[String]) -> Option<Request> {
    let args = expand(args);
    let mut request = Request::default();
    let mut i = 0;
    // `--flag value` and `--flag=value` alike.
    let value = |i: &mut usize, flag: &str| -> Option<String> {
        let arg = &args[*i];
        if arg == flag {
            *i += 1;
            return args.get(*i).cloned();
        }
        arg.strip_prefix(&format!("{flag}=")).map(str::to_owned)
    };
    while i < args.len() {
        if let Some(v) = value(&mut i, "--crate-name") {
            request.crate_name = v;
        } else if let Some(v) = value(&mut i, "--crate-type") {
            request.crate_types.push(v);
        } else if let Some(v) = value(&mut i, "--out-dir") {
            request.out_dir = Some(PathBuf::from(v));
        } else if let Some(v) = value(&mut i, "--emit") {
            request.emit.extend(
                v.split(',')
                    .map(|e| e.split('=').next().unwrap_or_default().to_owned()),
            );
        } else if let Some(v) = value(&mut i, "--extern") {
            if let Some((_, path)) = v.rsplit_once('=') {
                request.externs.push(path.to_owned());
            }
        } else if let Some(v) = value(&mut i, "--json") {
            request.json_artifacts |= v.split(',').any(|j| j == "artifacts");
        } else if args[i] == "-C" || args[i].starts_with("-C") {
            let codegen = if args[i] == "-C" {
                i += 1;
                args.get(i).cloned().unwrap_or_default()
            } else {
                args[i][2..].to_owned()
            };
            if let Some(extra) = codegen.strip_prefix("extra-filename=") {
                request.extra = extra.to_owned();
            }
        }
        i += 1;
    }
    (!request.crate_name.is_empty() && request.out_dir.is_some()).then_some(request)
}

/// What a compile was given as dependencies, named as the SDK names them: sorted.
pub fn externs_of(request: &Request) -> Vec<String> {
    let mut idents: Vec<String> = request
        .externs
        .iter()
        .filter_map(|path| crate::ident_of_path(path))
        .collect();
    idents.sort();
    idents
}

/// The marker the wrapper leaves beside a shared library it served.
fn marker(library: &Path) -> PathBuf {
    let mut name = library.file_name().unwrap_or_default().to_os_string();
    name.push(".sdk");
    library.with_file_name(name)
}

/// A shared library built on other shared libraries is only the SDK's if those were the SDK's too, not compiled
/// here with other features: each carries a marker.
fn shared_dependencies_were_served(request: &Request) -> bool {
    request
        .externs
        .iter()
        .filter(|path| crate::ident_of_path(path).is_some_and(|ident| ident.starts_with("dylib:")))
        .all(|path| marker(Path::new(path)).is_file())
}

/// Prints the notification Cargo waits for when it asked for `--json=artifacts`.
fn announce(request: &Request, path: &Path, emit: &str) {
    if request.json_artifacts {
        let path = serde_json::to_string(&path.to_string_lossy()).unwrap_or_default();
        eprintln!("{{\"$message_type\":\"artifact\",\"artifact\":{path},\"emit\":\"{emit}\"}}");
    }
}

fn read_slice(path: &Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    let (offset, len) = (offset as usize, len as usize);
    bytes
        .get(offset..offset + len)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stub shorter than recorded",
            )
        })
}

/// Writes what rustc would for a library crate: the dep-info file, the `.rmeta` and the `.rlib`.
fn serve_stub(
    request: &Request,
    dir: &Path,
    unit: &Unit,
    stub: &Stub,
    out: &Path,
) -> std::io::Result<()> {
    let base = format!("{}{}", request.crate_name, unit.extra);
    let (info, rmeta, rlib) = (
        out.join(format!("{base}.d")),
        out.join(format!("lib{base}.rmeta")),
        out.join(format!("lib{base}.rlib")),
    );
    let wants = |emit: &str| request.emit.iter().any(|e| e == emit);
    if wants("dep-info") {
        // No source files: Cargo never rebuilds this unit because a source changed.
        let text = format!(
            "{}: \n\n{}: \n\n{}: \n",
            info.display(),
            rlib.display(),
            rmeta.display()
        );
        fs::write(&info, text)?;
        announce(request, &info, "dep-info");
    }
    let stored = dir.join(&stub.stored);
    if wants("metadata") {
        match &stub.rmeta_stored {
            Some(file) => {
                fs::copy(dir.join(file), &rmeta)?;
            }
            None => fs::write(
                &rmeta,
                read_slice(&stored, stub.rmeta_offset, stub.rmeta_len)?,
            )?,
        }
        announce(request, &rmeta, "metadata");
    }
    if wants("link") {
        fs::copy(&stored, &rlib)?;
        announce(request, &rlib, "link");
    }
    Ok(())
}

/// Writes what rustc would for a shared library: the dep-info file and the library's own files.
fn serve_shared(request: &Request, dir: &Path, unit: &Unit, out: &Path) -> std::io::Result<()> {
    let info = out.join(format!("{}{}.d", request.crate_name, unit.extra));
    if request.emit.iter().any(|e| e == "dep-info") {
        fs::write(&info, format!("{}: \n", info.display()))?;
        announce(request, &info, "dep-info");
    }
    if request.emit.iter().any(|e| e == "link") {
        for file in &unit.copy {
            let to = out.join(&file.name);
            fs::copy(dir.join(&file.stored), &to)?;
            if crate::ident_of_path(&file.name).is_some_and(|ident| ident.starts_with("dylib:")) {
                fs::write(marker(&to), b"")?;
            }
            announce(request, &to, "link");
        }
    }
    Ok(())
}

/// Answers the compile from the SDK if it has the unit: `Some(exit code)`; `None` to run rustc.
pub fn serve(args: &[String]) -> Option<i32> {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os(SDK_ENV)?).collect();
    let request = parse(args)?;
    let index = Index::load(&dirs);
    let out = request.out_dir.clone()?;
    let types = &request.crate_types;
    let is_library = types.iter().all(|t| t == "lib" || t == "rlib");
    let is_shared = types.iter().any(|t| t == "dylib" || t == "cdylib");
    let candidates = index.get(&request.crate_name, &request.extra);
    let result = if is_library {
        let (dir, unit) = candidates.iter().find(|(_, unit)| unit.stub.is_some())?;
        serve_stub(&request, dir, unit, unit.stub.as_ref()?, &out)
    } else if types.iter().any(|t| t == "proc-macro") {
        // Same crate name and hash: the same proc-macro, built by the same compiler.
        let (dir, unit) = candidates.iter().find(|(_, unit)| {
            !unit.copy.is_empty() && unit.crate_types.iter().any(|t| t == "proc-macro")
        })?;
        serve_shared(&request, dir, unit, &out)
    } else if is_shared {
        let wanted = externs_of(&request);
        let (dir, unit) = candidates
            .iter()
            .find(|(_, unit)| !unit.copy.is_empty() && unit.externs == wanted)?;
        if !shared_dependencies_were_served(&request) {
            return None;
        }
        serve_shared(&request, dir, unit, &out)
    } else {
        return None;
    };
    let verbose = std::env::var_os(VERBOSE_ENV).is_some();
    match result {
        Ok(()) => {
            if verbose {
                eprintln!("sdk-rustc: served {}{}", request.crate_name, request.extra);
            }
            Some(0)
        }
        Err(error) => {
            // A broken SDK entry must not break the build: compile it.
            eprintln!(
                "sdk-rustc: {}{}: {error}; compiling it",
                request.crate_name, request.extra
            );
            None
        }
    }
}

/// `sdk-rustc <rustc> <arguments>`: the process's whole job.
pub fn main(args: Vec<String>) -> i32 {
    let Some((rustc, rest)) = args.split_first() else {
        eprintln!("sdk-rustc is a RUSTC_WRAPPER: cargo runs it as `sdk-rustc <rustc> <arguments>`");
        return 2;
    };
    if let Some(code) = serve(rest) {
        return code;
    }
    match Command::new(rustc).args(rest).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("sdk-rustc: cannot run {rustc}: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_cargo_compile_is_read() {
        let request = parse(&args(&[
            "--crate-name",
            "serde",
            "--edition=2021",
            "src/lib.rs",
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--crate-type",
            "lib",
            "--emit=dep-info,metadata,link",
            "-C",
            "metadata=abc",
            "-C",
            "extra-filename=-abc",
            "--out-dir",
            "target/debug/deps",
        ]))
        .unwrap();
        assert_eq!(request.crate_name, "serde");
        assert_eq!(request.extra, "-abc");
        assert_eq!(request.crate_types, ["lib"]);
        assert_eq!(request.emit, ["dep-info", "metadata", "link"]);
        assert!(request.json_artifacts);
        assert_eq!(request.out_dir, Some(PathBuf::from("target/debug/deps")));
    }

    #[test]
    fn a_version_query_is_not_a_compile() {
        assert_eq!(parse(&args(&["-vV"])), None);
        assert_eq!(parse(&args(&["--crate-name", "___", "--print=cfg"])), None);
    }
}

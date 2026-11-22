//! `cargo sdk`: cut an SDK from a build, inspect one, or run a command that uses it.
//!
//! ```text
//! cargo sdk pack --out <dir> --package <name>... [-- <cargo build arguments>]
//! cargo sdk fetch --repo <owner/name> [--tag <tag>] --asset <file> --out <dir>
//! cargo sdk exec --sdk <dir>... -- <command>
//! cargo sdk info <dir>
//! ```

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use cargo_sdk::manifest::Manifest;
use cargo_sdk::pack::{self, Options};
use cargo_sdk::{SDK_ENV, VERBOSE_ENV};

const USAGE: &str = "usage:
  cargo sdk pack --out <dir> [--archive <file.tar.gz>] --package <name>... [-- <cargo build arguments>]
      cut an SDK from the build `cargo build <arguments>` makes (a finished build is not rebuilt)
  cargo sdk exec --sdk <dir>... [--verbose] -- <command>
      run <command> (usually `cargo build ...`) with the wrapper serving the SDK
  cargo sdk info <dir>
      what an SDK holds
  cargo sdk fetch --repo <owner/name> [--tag <tag>] --asset <file.tar.gz> --out <dir>
      download an SDK from a GitHub release (the latest unless --tag) and unpack it";

fn fail(message: &str) -> ExitCode {
    eprintln!("cargo sdk: {message}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    // Cargo runs `cargo-sdk sdk <arguments>`.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("sdk") {
        args.remove(0);
    }
    let Some(command) = args.first().cloned() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    match command.as_str() {
        "pack" => run_pack(rest),
        "exec" => run_exec(rest),
        "info" => run_info(rest),
        "fetch" => run_fetch(rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run_pack(args: &[String]) -> ExitCode {
    let (mut roots, mut out, mut archive) = (Vec::new(), None, None);
    let mut cargo_args = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => out = iter.next().map(PathBuf::from),
            "--archive" => archive = iter.next().map(PathBuf::from),
            "--package" | "-p" => roots.extend(iter.next().cloned()),
            "--" => {
                cargo_args.extend(iter.by_ref().cloned());
            }
            other => return fail(&format!("unknown argument `{other}`")),
        }
    }
    let Some(out) = out else { return fail("pack needs --out <dir>") };
    if roots.is_empty() {
        return fail("pack needs at least one --package <name>");
    }
    match pack::pack(&Options { roots, cargo_args, out: out.clone() }) {
        Ok(summary) => {
            let mb = |b: u64| b as f64 / 1_048_576.0;
            eprintln!(
                "cargo sdk: {} library crates and {} shared libraries -> {} ({:.0} MB of rlibs stored as {:.0} MB)",
                summary.library_crates, summary.shared_libraries, out.display(), mb(summary.original_bytes), mb(summary.stored_bytes)
            );
            if let Some(archive) = archive {
                if let Err(error) = cargo_sdk::archive::create(&out, &archive) {
                    return fail(&error);
                }
                eprintln!("cargo sdk: archived as {}", archive.display());
            }
            if !summary.skipped.is_empty() {
                eprintln!("cargo sdk: left to the compiler: {} crates (proc-macros, build scripts, binaries)", summary.skipped.len());
                if std::env::var_os(VERBOSE_ENV).is_some() {
                    for skipped in &summary.skipped {
                        eprintln!("cargo sdk:   {skipped}");
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

fn run_exec(args: &[String]) -> ExitCode {
    let mut sdks = Vec::new();
    let mut verbose = false;
    let mut iter = args.iter();
    let mut command = Vec::new();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--sdk" => sdks.extend(iter.next().map(PathBuf::from)),
            "--verbose" => verbose = true,
            "--" => {
                command.extend(iter.by_ref().cloned());
            }
            other => return fail(&format!("unknown argument `{other}`")),
        }
    }
    let Some((program, program_args)) = command.split_first() else { return fail("exec needs a command after `--`") };
    // The wrapper sits beside this binary.
    let wrapper = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(format!("sdk-rustc{}", std::env::consts::EXE_SUFFIX))));
    let Some(wrapper) = wrapper.filter(|w| w.is_file()) else { return fail("sdk-rustc is not beside cargo-sdk") };
    let paths = match std::env::join_paths(&sdks) {
        Ok(paths) => paths,
        Err(error) => return fail(&error.to_string()),
    };
    let mut process = Command::new(program);
    process.args(program_args).env("RUSTC_WRAPPER", wrapper).env(SDK_ENV, paths);
    if verbose {
        process.env(VERBOSE_ENV, "1");
    }
    match process.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(error) => fail(&format!("cannot run {program}: {error}")),
    }
}

fn run_info(args: &[String]) -> ExitCode {
    let Some(dir) = args.first() else { return fail("info needs an SDK directory") };
    match Manifest::read(&PathBuf::from(dir)) {
        Ok(manifest) => {
            let stubs = manifest.units.iter().filter(|u| u.stub.is_some()).count();
            let shared = manifest.units.iter().filter(|u| !u.copy.is_empty()).count();
            println!("{} units: {stubs} library crates, {shared} shared libraries", manifest.units.len());
            println!("{}", manifest.rustc.lines().next().unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

fn run_fetch(args: &[String]) -> ExitCode {
    let (mut repo, mut tag, mut asset, mut out) = (None, "latest".to_owned(), None, None);
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--repo" => repo = iter.next().cloned(),
            "--tag" => tag = iter.next().cloned().unwrap_or(tag),
            "--asset" => asset = iter.next().cloned(),
            "--out" => out = iter.next().map(PathBuf::from),
            other => return fail(&format!("unknown argument `{other}`")),
        }
    }
    let (Some(repo), Some(asset), Some(out)) = (repo, asset, out) else {
        return fail("fetch needs --repo, --asset and --out");
    };
    let scratch = out.with_extension("download");
    let archive = match cargo_sdk::archive::download(&repo, &tag, &asset, &scratch) {
        Ok(path) => path,
        Err(error) => return fail(&error),
    };
    if let Err(error) = cargo_sdk::archive::extract(&archive, &out) {
        return fail(&error);
    }
    let _ = std::fs::remove_dir_all(&scratch);
    match Manifest::read(&out) {
        Ok(manifest) => {
            eprintln!("cargo sdk: {repo} {tag}: {} units in {}", manifest.units.len(), out.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

//! The whole idea on a toy workspace: a crate (`inner`) inside a shared library (`shared`), and an app that uses both.
//! Build the shared library, cut an SDK, then build the app in a fresh target directory with the wrapper and check
//! that it served both and that the app still runs (one copy of `inner`'s state, generics instantiated in the app).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn workspace() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cargo-sdk-toy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"inner\", \"shared\", \"app\"]\nresolver = \"2\"\n",
    );
    // The shared library shares `std` with its consumers.
    write(
        &root,
        ".cargo/config.toml",
        "[build]\nrustflags = [\"-C\", \"prefer-dynamic\"]\n",
    );
    write(
        &root,
        "inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &root,
        "inner/src/lib.rs",
        "use std::sync::atomic::{AtomicU32, Ordering};\nstatic COUNTER: AtomicU32 = AtomicU32::new(0);\npub fn next() -> u32 { COUNTER.fetch_add(1, Ordering::SeqCst) + 1 }\npub fn generic<T: std::fmt::Debug>(value: T) -> String { format!(\"{value:?}\") }\n",
    );
    write(&root, "shared/Cargo.toml", "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"dylib\"]\n[dependencies]\ninner = { path = \"../inner\" }\n");
    write(&root, "shared/src/lib.rs", "pub use inner;\n");
    write(&root, "app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ninner = { path = \"../inner\" }\nshared = { path = \"../shared\" }\n");
    write(&root, "app/src/main.rs", "use shared as _;\nfn main() {\n    println!(\"{} {} {}\", inner::next(), inner::next(), inner::generic(\"hi\"));\n}\n");
    root
}

fn cargo(root: &Path, target: &Path) -> Command {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.current_dir(root).env("CARGO_TARGET_DIR", target);
    command
}

#[test]
fn a_consumer_builds_from_the_sdk_and_runs() {
    let root = workspace();
    let (a, b, sdk) = (
        root.join("target-a"),
        root.join("target-b"),
        root.join("sdk"),
    );

    // 1. The shared library, built once.
    assert!(cargo(&root, &a)
        .args(["build", "--release", "-p", "shared"])
        .status()
        .unwrap()
        .success());

    // 2. The SDK, cut from that build.
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-sdk"))
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &a)
        .args(["sdk", "pack", "--out"])
        .arg(&sdk)
        .args(["--package", "shared", "--", "--release", "-p", "shared"])
        .status()
        .unwrap();
    assert!(status.success());

    // 3. A fresh target directory: the app is built with the wrapper serving the SDK.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-sdk"))
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &b)
        .args(["sdk", "exec", "--verbose", "--sdk"])
        .arg(&sdk)
        .args(["--", "cargo", "build", "--release", "-p", "app"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{log}");
    assert!(
        log.contains("served inner-"),
        "the library crate was compiled, not served:\n{log}"
    );
    assert!(
        log.contains("served shared"),
        "the shared library was compiled, not served:\n{log}"
    );

    // 4. It runs: both libraries are found, and `inner`'s state exists once.
    let libdir = String::from_utf8(
        Command::new("rustc")
            .args(["--print", "target-libdir"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let var = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let paths =
        std::env::join_paths([b.join("release").join("deps"), PathBuf::from(libdir.trim())])
            .unwrap();
    let run = Command::new(
        b.join("release")
            .join(format!("app{}", std::env::consts::EXE_SUFFIX)),
    )
    .env(var, paths)
    .output()
    .unwrap();
    let listing = |dir: PathBuf| -> String {
        fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "1 2 \"hi\"",
        "{}\n{var}={}\ndeps: {}\nrelease: {}",
        String::from_utf8_lossy(&run.stderr),
        std::env::join_paths([b.join("release").join("deps")])
            .unwrap()
            .to_string_lossy(),
        listing(b.join("release").join("deps")),
        listing(b.join("release")),
    );

    let _ = fs::remove_dir_all(&root);
}

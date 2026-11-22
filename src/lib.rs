//! cargo-sdk: precompiled crate SDKs for Cargo.
//!
//! A project that links a set of crates into a shared library (a Rust `dylib`) never needs those crates'
//! machine code when it builds a consumer of the library: the code is in the library. It still needs each
//! crate's *metadata* (types, traits, generics), and Cargo gets that only by compiling the crate. This
//! crate lets one build publish that metadata (an *SDK*) and every other build reuse it:
//!
//! - [`pack`] reads a finished build and stores, for every crate inside the library, a *stub rlib*: the
//!   rlib with its machine code removed, which is all rustc reads of a crate inside a linked dylib. It also
//!   stores the shared libraries themselves.
//! - [`wrapper`] is the `RUSTC_WRAPPER` (`sdk-rustc`): when Cargo asks rustc to compile a crate the SDK has,
//!   it writes the stored outputs and answers as rustc would, so Cargo believes the compile ran.
//!
//! See the README for how the pieces are used, and `docs/how-it-works.md` for what is and isn't safe.

pub mod archive;
pub mod manifest;
pub mod pack;
pub mod rlib;
pub mod wrapper;

/// The environment variable naming the SDK directories the wrapper may use (a path list).
pub const SDK_ENV: &str = "CARGO_SDK_DIR";

/// The environment variable that makes the wrapper print what it served and what it passed through.
pub const VERBOSE_ENV: &str = "CARGO_SDK_VERBOSE";

/// Names a file Cargo hands to rustc as a dependency (`--extern name=<path>`) or reports as an artifact, the same
/// way from both sides: `rlib:<name>-<hash>` for a library crate (`lib<name>-<hash>.rlib` or `.rmeta`),
/// `proc:<name>-<hash>` for a proc-macro (`<name>-<hash>.dll`), and `dylib:<name>` for a shared library, whose
/// file name carries no hash.
pub fn ident_of_path(path: &str) -> Option<String> {
    let file = path.rsplit(|c| c == '/' || c == '\\').next()?;
    let hashed = |stem: &str| {
        let (_, hash) = stem.rsplit_once('-')?;
        (hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| stem.to_owned())
    };
    for suffix in [".rlib", ".rmeta"] {
        if let Some(stem) = file.strip_suffix(suffix) {
            return hashed(stem.strip_prefix("lib").unwrap_or(stem)).map(|s| format!("rlib:{s}"));
        }
    }
    for suffix in [".dll", ".so", ".dylib"] {
        if let Some(stem) = file.strip_suffix(suffix) {
            let stem = stem.strip_prefix("lib").unwrap_or(stem);
            return Some(match hashed(stem) {
                Some(s) => format!("proc:{s}"),
                None => format!("dylib:{stem}"),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::ident_of_path;

    #[test]
    fn files_are_named_by_what_they_are() {
        assert_eq!(ident_of_path("target/deps/libserde-0123456789abcdef.rlib").as_deref(), Some("rlib:serde-0123456789abcdef"));
        assert_eq!(ident_of_path(r"C:\t\deps\libserde-0123456789abcdef.rmeta").as_deref(), Some("rlib:serde-0123456789abcdef"));
        assert_eq!(ident_of_path("deps/serde_derive-0123456789abcdef.dll").as_deref(), Some("proc:serde_derive-0123456789abcdef"));
        assert_eq!(ident_of_path("deps/wgpui_dylib.dll").as_deref(), Some("dylib:wgpui_dylib"));
        assert_eq!(ident_of_path("deps/libwgpui_dylib.so").as_deref(), Some("dylib:wgpui_dylib"));
        assert_eq!(ident_of_path("deps/readme.txt"), None);
    }
}

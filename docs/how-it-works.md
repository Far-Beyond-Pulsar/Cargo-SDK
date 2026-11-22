# How it works

## The problem

When crates are linked into a shared library (a Rust `dylib`), a program that *uses* the library does not need the
crates' machine code: it is in the library. But Cargo does not know that. It sees ordinary dependencies and compiles
every one of them, because rustc needs each crate's **metadata** (its types, traits and generic code) to compile the
code that uses it. A crate's metadata exists only as the output of compiling the crate.

For a large dependency graph that is most of the build, repeated by every consumer.

## What rustc actually reads

An rlib is an `ar` archive: a symbol table, `lib.rmeta` (the metadata), `lib.rmeta-link`, and the machine code. When a
crate is part of a dylib that is also being linked, rustc loads its metadata and never reads the machine code. So an
rlib with the code removed (a *stub rlib*) is a complete answer to "compile this crate" for any build that links the
dylib. The standalone `.rmeta` file Cargo passes between crates when it pipelines compilation is the same bytes as the
rlib's `lib.rmeta` member, so one stored file serves both.

## The pieces

**`cargo sdk pack`** runs (or reuses) a build and reads two things from Cargo: the dependency graph (`cargo metadata`)
and the artifact messages of the build (`--message-format=json`). For every library crate in the closure of the chosen
root packages it stores a stub rlib and records where the `.rmeta` sits inside it. For shared libraries it stores the
library files. The result is a directory with a `manifest.json`, optionally archived as a `.tar.gz` for a release.

**`sdk-rustc`** is a `RUSTC_WRAPPER`. Cargo runs it as `sdk-rustc <rustc> <arguments>` for every crate. It reads the
arguments, and if the SDK has a unit with the same crate name and the same `-C extra-filename` hash it writes the files
rustc would have written (`lib<name>-<hash>.rlib`, `.rmeta`, the dep-info file) and prints the JSON artifact messages
Cargo waits for; otherwise it runs the real rustc.

## Why answering by that key is safe

Cargo derives the hash in every output file name from everything that can change a crate's compiled form: its source,
features, dependencies (their hashes too), profile, target, compiler flags and the compiler version. Two compiles with
the same crate name and hash are the same compile. If anything differs (a feature, a flag, a dependency version, a
toolchain) the hash differs, the SDK has no unit for it, and the wrapper compiles the crate normally. A stale or
mismatched SDK therefore costs build time; it cannot produce a wrong build.

Shared libraries are the exception: their output files carry no hash. They are keyed by name and by the exact files
they were compiled against (the `--extern` arguments, which do carry hashes), and a shared library built on another is
only served if that one was served too.

## What it cannot do

- **Proc-macros and build scripts are still compiled.** They run on the build machine and are not part of the library.
  For crates that compile C code in a build script, that can be a large share of what is left.
- **An SDK is for one target and one toolchain.** Build one per platform, with the compiler you build with.
- **The feature set must be identical.** A shared library's contents depend on the features Cargo unifies across the
  build; if consumers can enable different features than the build the SDK was cut from, nothing matches. Pin them with a
  generated "workspace-hack" crate that depends on the library's crates with the union of features and is included in
  every build (this is what `cargo-hakari` generates).
- **Nothing is checked at run time.** The wrapper trusts the hash. Use an SDK only from builds you trust, as you would a
  cache.

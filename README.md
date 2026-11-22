# cargo-sdk

Precompiled crate SDKs for Cargo: build the crates inside a shared library once, and let every other build skip them.

## The problem

If your project links a set of crates into a shared library (a Rust `dylib`), every program that uses that library
still has Cargo compile each of those crates again. The machine code is already in the library; Cargo recompiles the
crates only because rustc needs their metadata to compile the code that uses them.

`cargo-sdk` stores that metadata (a *SDK*) and hands it back to Cargo on later builds.

## Where it is useful

- A plugin system whose plugins and host share one set of dylibs (many plugin repos, one toolchain).
- A monorepo where several binaries link the same large dependency set as a dylib.
- CI that builds many small consumers of one big library.

It is not useful when nothing is shared as a dylib: for ordinary rlib dependencies use `sccache` or Cargo's own cache.

## Install

```
cargo install --git <this repository> cargo-sdk
```

or download `cargo-sdk-<platform>.tar.gz` from a release and put `cargo-sdk` and `sdk-rustc` in the same directory.

## Use

Cut an SDK from a finished build (the arguments after `--` are what you would give `cargo build`):

```
cargo sdk pack --out sdk --package my_shared_dylib -- --release -p my_shared_dylib
cargo sdk pack --out sdk --archive ...          # also writes sdk.tar.gz for a release
```

Build with it:

```
cargo sdk exec --sdk sdk -- cargo build --release -p my_plugin
cargo sdk exec --sdk sdk --verbose -- cargo build ...   # prints what was served and what was compiled
```

Fetch one from a GitHub release (uses `gh` if present, else `curl`):

```
cargo sdk fetch --repo owner/name --tag v1.2.3 --asset sdk.tar.gz --out sdk
cargo sdk info sdk
```

`exec` only sets two environment variables: `RUSTC_WRAPPER` (the `sdk-rustc` beside `cargo-sdk`) and `CARGO_SDK_DIR`
(the SDK directories, a path list). You can set them yourself in CI.

## Safety

The wrapper answers a compile only when the crate name and Cargo's `-C extra-filename` hash match a stored unit. That
hash covers source, features, dependencies, profile, target, flags and compiler version, so a mismatched SDK is simply
not used and the crate is compiled as usual. See [docs/how-it-works.md](docs/how-it-works.md).

## Limitations

- Proc-macros and build scripts still run.
- One SDK per target triple and toolchain.
- Feature sets must be identical between the build the SDK was cut from and the consumer (pin with a workspace-hack crate).

## Benchmarks

To be filled with measured numbers from a real project before publishing.

## Development

```
cargo test --release     # includes a toy workspace built end to end
```

MIT licensed.

# Cargo SDK

Precompiled crate SDKs for Cargo: compile a set of crates once, publish the result, and let every other build skip
them.

## 2.9x faster builds: 10m 39s down to 3m 42s

A real plugin from a large Rust workspace (a game engine editor, 852 crates in its dependency graph), built from an
empty target directory on the same machine, release profile, identical command:

|  | Wall time | |
|---|---|---|
| Cargo, as usual | **640 s** (10m 39s) | 852 crates compiled |
| Cargo with an SDK | **222 s** (3m 42s) | 858 compiler invocations answered from the SDK |

That is **418 seconds saved per build, 65% less**, and the output is the same size to the byte
(`blueprint_editor_plugin.dll` 24,025,600 bytes both ways). The 222 s left is what an SDK does not cover yet: running
build scripts, compiling the plugin's own code and linking. Details, caveats and the earlier failed attempts are in
[docs/benchmarks.md](docs/benchmarks.md).

One machine, one run each, Windows x86_64: treat the ratio as indicative, not as a guarantee for your project.

## The problem

Every CI job and every fresh checkout of a big workspace compiles the same hundreds of dependencies again. When the
crates are also linked into a shared library (a Rust `dylib`) the waste is worse: the machine code is already in the
library, and Cargo recompiles the crates anyway, because rustc needs their metadata to compile the code that uses
them.

`cargo-sdk` stores what those compiles produce (an *SDK*) and hands it back to Cargo on later builds, without Cargo
knowing: it plugs in as Cargo's `RUSTC_WRAPPER`.

## Where it is useful

- A plugin system whose plugins and host share a set of dylibs (many plugin repositories, one toolchain): publish the
  SDK with the libraries' release and every plugin pipeline downloads it instead of compiling the libraries' crates.
- A monorepo or a family of repositories that build against the same large dependency set.
- CI that builds many consumers of one big library.

## Install

Download `cargo-sdk-<platform>.tar.gz` from [Releases](../../releases) and put `cargo-sdk` and `sdk-rustc` in the same
directory on your `PATH` (six platforms: Linux x86_64, Windows x86_64/i686/arm64, macOS x86_64/arm64), or in GitHub
Actions:

```yaml
- uses: Far-Beyond-Pulsar/Cargo-SDK@main   # downloads the release binaries, compiles nothing
```

or from source: `cargo install --git https://github.com/Far-Beyond-Pulsar/Cargo-SDK cargo-sdk`.

## Use

Cut an SDK from a build (the arguments after `--` are what you would give `cargo build`; a finished build is not
rebuilt):

```
cargo sdk pack --out sdk --package my_shared_dylib -- --release -p my_plugin -p my_shared_dylib
cargo sdk pack --out sdk --archive sdk.tar.gz ...      # also writes an archive for a release
```

`--package` names the roots whose dependency closure the SDK covers. Build with it:

```
cargo sdk exec --sdk sdk -- cargo build --release -p my_plugin
cargo sdk exec --sdk sdk --verbose -- cargo build ...  # prints what was served
```

Fetch one from a GitHub release (uses `gh` if present, else `curl`):

```
cargo sdk fetch --repo owner/name --tag v1.2.3 --asset sdk.tar.gz --out sdk
cargo sdk info sdk
```

`exec` only sets two environment variables: `RUSTC_WRAPPER` (the `sdk-rustc` beside `cargo-sdk`) and `CARGO_SDK_DIR`
(the SDK directories, a path list). You can set them yourself in CI.

An SDK stores each crate's whole rlib and its metadata, and proc-macros as built, so it is correct for every use of a
crate (build scripts and proc-macros link the same crates). `--strip` stores metadata only, a third smaller, for a
consumer that links the library's dylib and runs no build script or proc-macro against those crates.

## Safety

The wrapper answers a compile only when the crate name and Cargo's `-C extra-filename` hash match a stored unit. That
hash covers source, features, dependencies, profile, target, flags and compiler version, so a mismatched SDK is simply
not used and the crate is compiled as usual. See [docs/how-it-works.md](docs/how-it-works.md). The wrapper trusts the
hash: use an SDK only from builds you trust, as you would any build cache.

## Limitations

- Build scripts still compile and run. Replaying them is the next step.
- One SDK per target triple and toolchain.
- Feature sets must be identical between the build the SDK was cut from and the consumer, or the hashes differ and
  nothing is served. Pin them with a "workspace hack" crate (`cargo-hakari` makes one).
- A release SDK of a large graph is big: the one above is 3.3 GB unpacked and 986 MB compressed. GitHub release
  assets are limited to 2 GiB.

## Development

```
cargo test --release     # includes a toy workspace built end to end; CI runs it on Linux, macOS and Windows
```

MIT licensed.

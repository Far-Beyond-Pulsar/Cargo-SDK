# Benchmarks

## Headline

One plugin of a large Rust workspace, built from an empty target directory, release profile, the same command both
times (`cargo build --release -p blueprint_editor_plugin --features blueprint_editor_plugin/dll -p pulsar_workspace_hack`),
Windows 11 x86_64, one machine, one run each:

| | Wall time | Crates |
|---|---|---|
| Cargo, as usual | 640 s (10m 39s) | 852 compiled |
| Cargo with an SDK (`cargo sdk exec`) | 222 s (3m 42s) | 858 compiler invocations served from the SDK |

2.9x faster, 418 s saved, 65% less. Outputs of the two builds have identical sizes: `blueprint_editor_plugin.dll`
24,025,600 bytes, `helio_dylib.dll` 4,855,808, `wgpui_dylib.dll` 70,275,584. (Sizes were compared, not contents.)

The SDK was cut from a build of the same workspace with the same compiler, 6 seconds after that build finished.
Size: 3.3 GB unpacked (full rlibs, 51 proc-macros and 7 shared libraries), 986 MB as `tar.gz`. What is left in the 222 s
is what the SDK does not cover: running build scripts, the plugin's own crates and linking.

## How it got here (what failed first)

The first real run is worth keeping, because it shows what an SDK must contain.

1. **Metadata-only stubs: failed.** The first design stored each crate's rlib without its machine code (a *stub*),
   which is all rustc reads of a crate linked from a dylib. The build failed at link time: build scripts and proc-macros
   (`num-traits`' build script, `serde_derive`) link against the same crates (`autocfg`, `proc_macro2`, `quote`) and need
   the code. An SDK now stores whole rlibs by default; `--strip` keeps the stubs for consumers that need nothing else.
2. **Whole rlibs, proc-macros compiled fresh: failed.** 649 crates were served, then `rustc` said it could not find
   `rust_i18n_support`. The served rlib records the metadata hash of the proc-macro it was built against (`rustversion`),
   and a proc-macro compiled again in another target directory has a different one (build-script output paths are part
   of it). Swapping the original proc-macro DLL in made the same `rustc` command succeed. An SDK now stores and serves
   proc-macros too.
3. **Whole rlibs and proc-macros: 222 s.**

## What these numbers do not show

- One run each, no repetitions, one machine, Windows x86_64 only. Linux and macOS are exercised only by the toy
  workspace test in CI.
- The consumer build matched the SDK exactly: same workspace state, same arguments, same toolchain. When the feature
  sets of two cargo invocations differ, crate hashes differ and nothing is served. In the same project, alternating a
  libraries-only build and a full build recompiled 49 and 236 crates, so the SDK must be cut from, and consumed with,
  one pinned feature set (a workspace-hack crate).
- The cold build here includes the cost of every dependency; incremental builds are not what an SDK speeds up.

## Toy workspace

`tests/toy_workspace.rs` builds a two-crate library as a shared library, cuts an SDK, builds a consumer in a fresh
target directory under the wrapper, checks that both the library crate and the shared library were served, and runs
the result. It passes on Linux, macOS and Windows in CI.

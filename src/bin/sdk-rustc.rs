//! The `RUSTC_WRAPPER`: see `cargo_sdk::wrapper`.

fn main() {
    std::process::exit(cargo_sdk::wrapper::main(std::env::args().skip(1).collect()));
}

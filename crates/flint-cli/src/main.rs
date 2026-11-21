//! `flint`: the engine's CLI with no extensions. A game binary that adds
//! commands of its own calls `flint_cli::run_with` with them (ADR 0133).

fn main() -> std::process::ExitCode {
    flint_cli::run_with(Vec::new())
}

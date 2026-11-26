//! `flint`: the CLI launcher with the stock list, every package this build
//! has (`flint_cli::stock`, #1553). A game binary that adds commands of its
//! own calls `flint_cli::run_with` with them (ADR 0133).

fn main() -> std::process::ExitCode {
    flint_cli::run_with(Vec::new())
}

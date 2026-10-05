//! The `lca` binary entry point (ADR-0002: `main` stays thin).

#![forbid(unsafe_code)]

use clap::Parser;

fn main() {
    // gh #19: before anything is written, so a pipe whose reader has
    // already gone (`| head`, a pager closing) ends the process by signal
    // instead of panicking on the next `println!`.
    lca_cli::sigpipe::restore_default();
    // #96: before anything that can panic, on every build profile — the
    // runtime runs the hook before unwinding or aborting, so a crash
    // leaves a usable terminal instead of a hidden cursor in alt screen.
    lca_tui::install_panic_hook();
    let cli = lca_cli::Cli::parse();
    // #152: before anything that can warn, so no diagnostic evaporates.
    // A diagnostics failure is reported, never fatal.
    if let Err(err) = lca_cli::init_diagnostics(cli.verbose) {
        eprintln!("warning: diagnostics disabled: {err}");
    }
    let code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(lca_cli::run(cli)),
        Err(err) => {
            eprintln!("error: cannot start the async runtime: {err}");
            1
        }
    };
    std::process::exit(code);
}

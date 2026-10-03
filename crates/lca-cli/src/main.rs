//! The `lca` binary entry point (ADR-0002: `main` stays thin).

#![forbid(unsafe_code)]

use clap::Parser;

fn main() {
    // gh #19: before anything is written, so a pipe whose reader has
    // already gone (`| head`, a pager closing) ends the process by signal
    // instead of panicking on the next `println!`.
    lca_cli::sigpipe::restore_default();
    let cli = lca_cli::Cli::parse();
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

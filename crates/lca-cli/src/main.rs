//! The `lca` binary entry point (ADR-0002: `main` stays thin).

use clap::Parser;

fn main() {
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

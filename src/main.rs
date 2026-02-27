use std::process;

use clap::Parser;

use arcpack::cli::{Cli, Commands};
use arcpack::cli::common::init_tracing;

fn main() {
    let cli = Cli::parse();

    init_tracing(cli.verbosity);

    let result = match cli.command {
        Commands::Plan(args) => {
            arcpack::cli::plan::run_plan(&args).map(|_| true)
        }
        Commands::Info(args) => {
            arcpack::cli::info::run_info(&args)
        }
        Commands::Schema => {
            arcpack::cli::schema::run_schema().map(|_| true)
        }
        Commands::Prepare(args) => {
            arcpack::cli::prepare::run_prepare(&args)
        }
        Commands::Build(args) => {
            arcpack::cli::build::run_build(&args)
        }
        #[cfg(feature = "grpc")]
        Commands::Frontend(args) => {
            arcpack::cli::frontend::run_frontend(&args)
        }
    };

    match result {
        Ok(true) => {}
        Ok(false) => process::exit(1),
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    }
}

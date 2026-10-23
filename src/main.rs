use clap::Parser;
use orbit_tracker::cli::Cli;
use orbit_tracker::{commands, output};

fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;

    if matches!(&cli.command, orbit_tracker::cli::Command::Mcp) {
        if let Err(error) = orbit_tracker::mcp::serve(cli.dir.as_deref()) {
            eprintln!("orbit mcp: {error}");
            std::process::exit(1);
        }
        return;
    }

    match commands::run(cli) {
        Ok(result) => {
            let exit_code = result.exit_code;
            output::print_success(result, json_mode);
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Err(error) => {
            output::print_error(&error, json_mode);
            std::process::exit(1);
        }
    }
}

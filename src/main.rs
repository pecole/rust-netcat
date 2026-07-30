mod cli;
mod tcp;
mod udp;

use clap::Parser;
use cli::Cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let target = match cli.resolve_target() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if cli.exec.is_some() && cli.udp {
        eprintln!("error: -e/--exec is not supported together with -u/--udp");
        return ExitCode::FAILURE;
    }

    let result = if cli.listen {
        if cli.udp {
            udp::run_server(&target.addr, target.port, cli.timeout, cli.verbose)
        } else {
            tcp::run_server(
                &target.addr,
                target.port,
                cli.keep_open,
                cli.exec.as_deref(),
                cli.verbose,
            )
        }
    } else if cli.udp {
        udp::run_client(&target.addr, target.port, cli.timeout, cli.verbose)
    } else {
        tcp::run_client(
            &target.addr,
            target.port,
            cli.timeout,
            cli.exec.as_deref(),
            cli.verbose,
        )
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

mod broadcast;
mod cli;
mod instrument;
mod socks5;
mod stream;
mod tcp;
mod tls;
mod udp;

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use regex::Regex;

use cli::Cli;
use instrument::{Instrumentation, JsonLogger, RateLimiter, Stats};

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Err(e) = cli.validate() {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    let target = match cli.resolve_target() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let json_log = match &cli.json_log {
        Some(path) => match JsonLogger::open(path) {
            Ok(logger) => Some(Arc::new(logger)),
            Err(e) => {
                eprintln!("error: could not open --json-log file: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let instrumentation = Instrumentation::configured(
        cli.hex,
        cli.timestamp,
        if cli.stats { Some(Arc::new(Stats::new())) } else { None },
        json_log,
        cli.rate_limit.map(|limit| Arc::new(RateLimiter::new(limit))),
    );

    // Already validated as compilable in Cli::validate().
    let filter = cli.filter.as_deref().map(|p| Regex::new(p).expect("filter regex validated"));

    let result = if cli.listen {
        if cli.broadcast {
            broadcast::run(&target.addr, target.port, cli.verbose, instrumentation)
        } else if cli.udp {
            udp::run_server(&udp::ServerOptions {
                bind_addr: &target.addr,
                port: target.port,
                timeout: cli.timeout,
                verbose: cli.verbose,
                filter: filter.as_ref(),
                instrumentation,
            })
        } else {
            tcp::run_server(&tcp::ServerOptions {
                bind_addr: &target.addr,
                port: target.port,
                keep_open: cli.keep_open,
                exec: cli.exec.as_deref(),
                verbose: cli.verbose,
                tls: cli.tls,
                tls_cert: cli.tls_cert.as_deref(),
                tls_key: cli.tls_key.as_deref(),
                filter: filter.as_ref(),
                instrumentation,
            })
        }
    } else if cli.udp {
        udp::run_client(&udp::ClientOptions {
            host: &target.addr,
            port: target.port,
            timeout: cli.timeout,
            verbose: cli.verbose,
            filter: filter.as_ref(),
            instrumentation,
        })
    } else {
        tcp::run_client(&tcp::ClientOptions {
            host: &target.addr,
            port: target.port,
            timeout: cli.timeout,
            exec: cli.exec.as_deref(),
            verbose: cli.verbose,
            proxy: cli.proxy.as_deref(),
            tls: cli.tls,
            tls_insecure: cli.tls_insecure,
            tls_ca: cli.tls_ca.as_deref(),
            retry: cli.retry,
            retry_delay: cli.retry_delay,
            filter: filter.as_ref(),
            instrumentation,
        })
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

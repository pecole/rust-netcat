use clap::Parser;

/// A netcat clone written in Rust.
///
/// Client mode:  rnc [OPTIONS] <HOST> <PORT>
/// Server mode:  rnc -l [OPTIONS] [BIND_ADDR] <PORT>   (or: rnc -l -p <PORT>)
#[derive(Parser, Debug)]
#[command(name = "rnc", author, version, about, long_about = None)]
pub struct Cli {
    /// Listen for an incoming connection instead of connecting out
    #[arg(short = 'l', long)]
    pub listen: bool,

    /// Use UDP instead of TCP
    #[arg(short = 'u', long)]
    pub udp: bool,

    /// Verbose output. Repeat for more detail (-v, -vv)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Server mode only: keep listening for further connections after one ends
    #[arg(short = 'k', long = "keep-open")]
    pub keep_open: bool,

    /// Port to listen on. Alternative to passing the port positionally
    #[arg(short = 'p', long = "port", value_name = "PORT")]
    pub port_flag: Option<u16>,

    /// Connection timeout in seconds (client mode) and idle read timeout
    #[arg(short = 'w', long)]
    pub timeout: Option<u64>,

    /// Execute a command and wire its stdio to the connection (TCP only).
    /// WARNING: this exposes a shell to whoever connects. Use only in
    /// authorized testing/lab environments.
    #[arg(short = 'e', long)]
    pub exec: Option<String>,

    /// Destination host (client mode) or bind address (server mode, default 0.0.0.0)
    pub host: Option<String>,

    /// Destination port (client mode) or listen port (server mode)
    pub port: Option<u16>,
}

/// Resolved (address, port) pair plus mode flags, derived from the raw CLI args.
#[derive(Debug, PartialEq)]
pub struct Target {
    pub addr: String,
    pub port: u16,
}

impl Cli {
    /// Work out the effective bind address/port (server) or host/port (client),
    /// accepting the handful of equivalent ways netcat lets you specify them:
    ///   rnc -l -p 4444
    ///   rnc -l 4444
    ///   rnc -l 0.0.0.0 4444
    ///   rnc example.com 4444
    pub fn resolve_target(&self) -> Result<Target, String> {
        if self.listen {
            if let Some(port) = self.port_flag {
                let addr = self.host.clone().unwrap_or_else(|| "0.0.0.0".to_string());
                return Ok(Target { addr, port });
            }
            if let Some(port) = self.port {
                let addr = self.host.clone().unwrap_or_else(|| "0.0.0.0".to_string());
                return Ok(Target { addr, port });
            }
            if let Some(host) = &self.host {
                // Only one positional was given in listen mode: treat it as the port.
                let port: u16 = host
                    .parse()
                    .map_err(|_| format!("invalid port number: {host}"))?;
                return Ok(Target {
                    addr: "0.0.0.0".to_string(),
                    port,
                });
            }
            Err("listen mode requires a port (-p <PORT> or a positional PORT)".to_string())
        } else {
            let addr = self
                .host
                .clone()
                .ok_or_else(|| "missing destination host".to_string())?;
            let port = self
                .port
                .or(self.port_flag)
                .ok_or_else(|| "missing destination port".to_string())?;
            Ok(Target { addr, port })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Cli {
        Cli {
            listen: false,
            udp: false,
            verbose: 0,
            keep_open: false,
            port_flag: None,
            timeout: None,
            exec: None,
            host: None,
            port: None,
        }
    }

    #[test]
    fn client_needs_host_and_port() {
        let mut cli = base();
        assert!(cli.resolve_target().is_err());
        cli.host = Some("example.com".to_string());
        assert!(cli.resolve_target().is_err());
        cli.port = Some(4444);
        let target = cli.resolve_target().unwrap();
        assert_eq!(
            target,
            Target {
                addr: "example.com".to_string(),
                port: 4444
            }
        );
    }

    #[test]
    fn client_port_flag_is_accepted_as_fallback() {
        let mut cli = base();
        cli.host = Some("example.com".to_string());
        cli.port_flag = Some(4444);
        let target = cli.resolve_target().unwrap();
        assert_eq!(target.port, 4444);
    }

    #[test]
    fn listen_with_port_flag_defaults_bind_addr() {
        let mut cli = base();
        cli.listen = true;
        cli.port_flag = Some(4444);
        let target = cli.resolve_target().unwrap();
        assert_eq!(
            target,
            Target {
                addr: "0.0.0.0".to_string(),
                port: 4444
            }
        );
    }

    #[test]
    fn listen_with_single_positional_treats_it_as_port() {
        let mut cli = base();
        cli.listen = true;
        cli.host = Some("4444".to_string());
        let target = cli.resolve_target().unwrap();
        assert_eq!(
            target,
            Target {
                addr: "0.0.0.0".to_string(),
                port: 4444
            }
        );
    }

    #[test]
    fn listen_with_bind_addr_and_port_positionals() {
        let mut cli = base();
        cli.listen = true;
        cli.host = Some("127.0.0.1".to_string());
        cli.port = Some(4444);
        let target = cli.resolve_target().unwrap();
        assert_eq!(
            target,
            Target {
                addr: "127.0.0.1".to_string(),
                port: 4444
            }
        );
    }

    #[test]
    fn listen_without_any_port_errors() {
        let mut cli = base();
        cli.listen = true;
        assert!(cli.resolve_target().is_err());
    }

    #[test]
    fn listen_with_invalid_single_positional_errors() {
        let mut cli = base();
        cli.listen = true;
        cli.host = Some("not-a-port".to_string());
        assert!(cli.resolve_target().is_err());
    }
}

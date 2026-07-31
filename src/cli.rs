use std::path::PathBuf;

use clap::Parser;

/// A netcat clone written in Rust.
///
/// Client mode:  rnc [OPTIONS] <HOST> <PORT>
/// Server mode:  rnc -l [OPTIONS] [BIND_ADDR] <PORT>   (or: rnc -l -p <PORT>)
#[derive(Parser, Debug, Default)]
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

    /// Hex-dump both directions of traffic to stderr (xxd-style)
    #[arg(short = 'x', long = "hex")]
    pub hex: bool,

    /// Print a transfer summary (bytes, duration, throughput) when the connection ends
    #[arg(long)]
    pub stats: bool,

    /// Add elapsed-time markers to --hex output
    #[arg(long)]
    pub timestamp: bool,

    /// Append a JSON-lines event log (connect/disconnect/data/error) to FILE
    #[arg(long, value_name = "FILE")]
    pub json_log: Option<PathBuf>,

    /// Only forward received lines matching REGEX to output (receive-side display filter)
    #[arg(long, value_name = "REGEX")]
    pub filter: Option<String>,

    /// Cap combined throughput to roughly BYTES_PER_SEC
    #[arg(long, value_name = "BYTES_PER_SEC")]
    pub rate_limit: Option<u64>,

    /// Client mode: retry the connection up to N times if it fails
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub retry: u32,

    /// Client mode: seconds to wait between retries
    #[arg(long, value_name = "SECS", default_value_t = 1)]
    pub retry_delay: u64,

    /// Server mode: relay messages between all connected clients (chat-room
    /// style) instead of wiring the connection to this process's stdio
    #[arg(long)]
    pub broadcast: bool,

    /// Client mode: connect through a SOCKS5 proxy at HOST:PORT
    #[arg(long, value_name = "HOST:PORT")]
    pub proxy: Option<String>,

    /// Wrap the connection in TLS
    #[arg(long)]
    pub tls: bool,

    /// Client TLS: skip server certificate verification (INSECURE, testing only)
    #[arg(long)]
    pub tls_insecure: bool,

    /// Client TLS: trust only the CA certificate(s) in FILE instead of the system trust store
    #[arg(long, value_name = "FILE")]
    pub tls_ca: Option<PathBuf>,

    /// Server TLS: certificate chain PEM file
    #[arg(long, value_name = "FILE")]
    pub tls_cert: Option<PathBuf>,

    /// Server TLS: private key PEM file
    #[arg(long, value_name = "FILE")]
    pub tls_key: Option<PathBuf>,

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

    /// Reject flag combinations that don't make sense together, before we
    /// touch the network. Keeping this in one place makes the actual
    /// client/server code paths free to assume a consistent configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.exec.is_some() && self.udp {
            return Err("-e/--exec is not supported together with -u/--udp".to_string());
        }
        if self.tls && self.udp {
            return Err("--tls is not supported together with -u/--udp".to_string());
        }
        if self.broadcast {
            if !self.listen {
                return Err("--broadcast requires -l/--listen".to_string());
            }
            if self.udp {
                return Err("--broadcast is not supported together with -u/--udp".to_string());
            }
            if self.exec.is_some() {
                return Err("--broadcast is not supported together with -e/--exec".to_string());
            }
        }
        if self.proxy.is_some() {
            if self.listen {
                return Err("--proxy is client-only (not valid with -l/--listen)".to_string());
            }
            if self.udp {
                return Err("--proxy only supports TCP (SOCKS5 CONNECT), not -u/--udp".to_string());
            }
        }
        if self.retry != 0 && self.listen {
            return Err("--retry is client-only (not valid with -l/--listen)".to_string());
        }
        if self.tls_insecure && !self.tls {
            return Err("--tls-insecure requires --tls".to_string());
        }
        if self.tls_ca.is_some() && !self.tls {
            return Err("--tls-ca requires --tls".to_string());
        }
        if (self.tls_cert.is_some() || self.tls_key.is_some()) && !self.listen {
            return Err("--tls-cert/--tls-key are server-only (not valid without -l/--listen)".to_string());
        }
        if self.tls && self.listen && (self.tls_cert.is_none() || self.tls_key.is_none()) {
            return Err("--tls server mode requires both --tls-cert and --tls-key".to_string());
        }
        if let Some(pattern) = &self.filter {
            regex::Regex::new(pattern).map_err(|e| format!("invalid --filter regex: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Cli {
        Cli::default()
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

    #[test]
    fn exec_with_udp_is_rejected() {
        let mut cli = base();
        cli.udp = true;
        cli.exec = Some("id".to_string());
        assert!(cli.validate().is_err());
    }

    #[test]
    fn broadcast_requires_listen() {
        let mut cli = base();
        cli.broadcast = true;
        assert!(cli.validate().is_err());
        cli.listen = true;
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn proxy_is_client_only() {
        let mut cli = base();
        cli.listen = true;
        cli.proxy = Some("127.0.0.1:1080".to_string());
        assert!(cli.validate().is_err());
    }

    #[test]
    fn tls_server_requires_cert_and_key() {
        let mut cli = base();
        cli.listen = true;
        cli.tls = true;
        assert!(cli.validate().is_err());
        cli.tls_cert = Some(PathBuf::from("cert.pem"));
        cli.tls_key = Some(PathBuf::from("key.pem"));
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn tls_insecure_requires_tls() {
        let mut cli = base();
        cli.tls_insecure = true;
        assert!(cli.validate().is_err());
    }

    #[test]
    fn invalid_filter_regex_is_rejected() {
        let mut cli = base();
        cli.filter = Some("(".to_string());
        assert!(cli.validate().is_err());
    }
}

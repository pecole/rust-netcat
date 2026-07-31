use std::io;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use regex::Regex;

use crate::instrument::{self, Direction, Instrumentation, LineFilter};
use crate::socks5;
use crate::stream::BoxedStream;
use crate::tls;

/// Build a "host:port" string that also works for bare IPv6 literals.
fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub struct ClientOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub timeout: Option<u64>,
    pub exec: Option<&'a str>,
    pub verbose: u8,
    pub proxy: Option<&'a str>,
    pub tls: bool,
    pub tls_insecure: bool,
    pub tls_ca: Option<&'a Path>,
    pub retry: u32,
    pub retry_delay: u64,
    pub filter: Option<&'a Regex>,
    pub instrumentation: Instrumentation,
}

pub struct ServerOptions<'a> {
    pub bind_addr: &'a str,
    pub port: u16,
    pub keep_open: bool,
    pub exec: Option<&'a str>,
    pub verbose: u8,
    pub tls: bool,
    pub tls_cert: Option<&'a Path>,
    pub tls_key: Option<&'a Path>,
    pub filter: Option<&'a Regex>,
    pub instrumentation: Instrumentation,
}

pub fn run_client(opts: &ClientOptions) -> io::Result<()> {
    let timeout = opts.timeout.map(Duration::from_secs);
    let tls_config = if opts.tls {
        Some(tls::client_config(opts.tls_insecure, opts.tls_ca)?)
    } else {
        None
    };

    let mut attempt = 0u32;
    let stream = loop {
        match connect_once(opts, timeout, tls_config.clone()) {
            Ok(s) => break s,
            Err(e) if attempt < opts.retry => {
                attempt += 1;
                if opts.verbose > 0 {
                    eprintln!(
                        "connect attempt {attempt}/{} failed: {e}; retrying in {}s",
                        opts.retry, opts.retry_delay
                    );
                }
                thread::sleep(Duration::from_secs(opts.retry_delay));
            }
            Err(e) => return Err(e),
        }
    };

    if opts.verbose > 0 {
        eprintln!("Connected to {}:{}", opts.host, opts.port);
    }
    if let Some(log) = &opts.instrumentation.json_log {
        log.log_connect(&format!("{}:{}", opts.host, opts.port));
    }

    let result = handle_connection(stream, opts.exec, opts.filter, &opts.instrumentation, opts.verbose);

    if let Some(log) = &opts.instrumentation.json_log {
        log.log_disconnect(&format!("{}:{}", opts.host, opts.port));
    }
    if let Some(stats) = &opts.instrumentation.stats {
        stats.print_summary();
    }

    result
}

fn connect_once(
    opts: &ClientOptions,
    timeout: Option<Duration>,
    tls_config: Option<Arc<rustls::ClientConfig>>,
) -> io::Result<BoxedStream> {
    let tcp_stream = if let Some(proxy_addr) = opts.proxy {
        if opts.verbose > 0 {
            eprintln!("Connecting via SOCKS5 proxy {proxy_addr}");
        }
        socks5::connect_via_proxy(proxy_addr, opts.host, opts.port, timeout)?
    } else {
        let addr_str = format_addr(opts.host, opts.port);
        match timeout {
            Some(t) => {
                let sockaddr = addr_str.to_socket_addrs()?.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "could not resolve address")
                })?;
                TcpStream::connect_timeout(&sockaddr, t)?
            }
            None => TcpStream::connect(&addr_str)?,
        }
    };

    if let Some(t) = timeout {
        tcp_stream.set_read_timeout(Some(t))?;
    }

    match tls_config {
        Some(config) => tls::wrap_client(tcp_stream, opts.host, config, timeout),
        None => Ok(Box::new(tcp_stream)),
    }
}

pub fn run_server(opts: &ServerOptions) -> io::Result<()> {
    let listener = TcpListener::bind((opts.bind_addr, opts.port))?;
    if opts.verbose > 0 {
        eprintln!("Listening on {}:{}", opts.bind_addr, opts.port);
    }

    let tls_config = if opts.tls {
        let cert = opts.tls_cert.expect("validated: --tls-cert required in server mode");
        let key = opts.tls_key.expect("validated: --tls-key required in server mode");
        Some(tls::server_config(cert, key)?)
    } else {
        None
    };

    loop {
        let (tcp_stream, peer) = listener.accept()?;
        if opts.verbose > 0 {
            eprintln!("Connection received from {peer}");
        }
        if let Some(log) = &opts.instrumentation.json_log {
            log.log_connect(&peer.to_string());
        }

        let stream: io::Result<BoxedStream> = match &tls_config {
            Some(config) => tls::wrap_server(tcp_stream, Arc::clone(config), None),
            None => Ok(Box::new(tcp_stream)),
        };

        match stream {
            Ok(stream) => {
                if let Err(e) = handle_connection(stream, opts.exec, opts.filter, &opts.instrumentation, opts.verbose) {
                    eprintln!("connection error: {e}");
                    if let Some(log) = &opts.instrumentation.json_log {
                        log.log_error(&e.to_string());
                    }
                }
            }
            Err(e) => {
                eprintln!("TLS handshake error: {e}");
                if let Some(log) = &opts.instrumentation.json_log {
                    log.log_error(&e.to_string());
                }
            }
        }

        if opts.verbose > 0 {
            eprintln!("Connection from {peer} closed");
        }
        if let Some(log) = &opts.instrumentation.json_log {
            log.log_disconnect(&peer.to_string());
        }
        if let Some(stats) = &opts.instrumentation.stats {
            stats.print_summary();
        }

        if !opts.keep_open {
            break;
        }
    }

    Ok(())
}

fn handle_connection(
    stream: BoxedStream,
    exec: Option<&str>,
    filter: Option<&Regex>,
    instrumentation: &Instrumentation,
    verbose: u8,
) -> io::Result<()> {
    match exec {
        Some(cmd) => exec_over_stream(stream, cmd, instrumentation, verbose),
        None => relay_stdio(stream, filter, instrumentation),
    }
}

/// Relay data between stdin/stdout and the connection, in both directions at once.
fn relay_stdio(mut stream: BoxedStream, filter: Option<&Regex>, instrumentation: &Instrumentation) -> io::Result<()> {
    let mut sock_in = stream.try_clone_boxed()?;
    let send_instrumentation = instrumentation.clone();

    let writer = thread::spawn(move || -> io::Result<u64> {
        let mut stdin = io::stdin();
        let n = instrument::pump(&mut stdin, &mut sock_in, Direction::Send, &send_instrumentation, None)?;
        let _ = sock_in.shutdown_write();
        Ok(n)
    });

    let line_filter = filter.map(|re| Mutex::new(LineFilter::new(re.clone())));
    let mut stdout = io::stdout();
    let recv_result = instrument::pump(&mut *stream, &mut stdout, Direction::Recv, instrumentation, line_filter.as_ref());

    let send_result = writer.join().unwrap_or(Ok(0));

    only_real_errors(recv_result)?;
    only_real_errors(send_result)?;
    Ok(())
}

/// A `-w` idle timeout or the peer closing the connection are the normal
/// ways a relay ends, not failures worth reporting; anything else (a TLS
/// handshake/certificate failure, a reset connection, ...) should surface
/// to the user instead of vanishing silently.
fn only_real_errors(result: io::Result<u64>) -> io::Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => Ok(()),
        Err(e) => Err(e),
    }
}

/// Spawn `cmd` in a shell and wire its stdin/stdout/stderr to the connection.
///
/// This is netcat's classic `-e` behavior. It hands a shell to whoever is on
/// the other end of the connection, so only use it against systems and
/// networks you're authorized to test (CTF boxes, your own lab, an
/// authorized pentest engagement).
fn exec_over_stream(mut stream: BoxedStream, cmd: &str, instrumentation: &Instrumentation, verbose: u8) -> io::Result<()> {
    if verbose > 0 {
        eprintln!("Executing `{cmd}` and wiring it to the connection");
    }

    let mut child = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
    } else {
        Command::new("sh")
            .args(["-c", cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
    };

    let mut child_stdin = child.stdin.take().expect("child stdin was piped");
    let mut child_stdout = child.stdout.take().expect("child stdout was piped");
    let mut child_stderr = child.stderr.take().expect("child stderr was piped");

    let mut sock_in = stream.try_clone_boxed()?;
    let mut sock_out = stream.try_clone_boxed()?;

    // Data arriving over the connection is fed to the shell's stdin (as if
    // the remote peer typed it); the shell's stdout/stderr go back out over
    // the connection. Instrumentation is tagged accordingly in each
    // direction and shares the same underlying stats/json-log/rate-limiter.
    let recv_instrumentation = instrumentation.clone();
    let t_in = thread::spawn(move || {
        let _ = instrument::pump(&mut sock_in, &mut child_stdin, Direction::Recv, &recv_instrumentation, None);
    });

    let send_instrumentation = instrumentation.clone();
    let t_out = thread::spawn(move || {
        let _ = instrument::pump(&mut child_stdout, &mut sock_out, Direction::Send, &send_instrumentation, None);
    });

    let send_instrumentation_err = instrumentation.clone();
    let t_err = thread::spawn(move || {
        let _ = instrument::pump(&mut child_stderr, &mut *stream, Direction::Send, &send_instrumentation_err, None);
    });

    let _ = child.wait();
    let _ = t_in.join();
    let _ = t_out.join();
    let _ = t_err.join();

    Ok(())
}

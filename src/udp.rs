use std::io::{self, Read, Write};
use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

use regex::Regex;

use crate::instrument::{Direction, Instrumentation, LineFilter};

const BUF_SIZE: usize = 65536;

pub struct ClientOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub timeout: Option<u64>,
    pub verbose: u8,
    pub filter: Option<&'a Regex>,
    pub instrumentation: Instrumentation,
}

pub struct ServerOptions<'a> {
    pub bind_addr: &'a str,
    pub port: u16,
    pub timeout: Option<u64>,
    pub verbose: u8,
    pub filter: Option<&'a Regex>,
    pub instrumentation: Instrumentation,
}

/// Build a "host:port" string that also works for bare IPv6 literals.
fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn run_client(opts: &ClientOptions) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let addr_str = format_addr(opts.host, opts.port);
    socket.connect(&addr_str)?;

    if let Some(secs) = opts.timeout {
        socket.set_read_timeout(Some(Duration::from_secs(secs)))?;
    }

    if opts.verbose > 0 {
        eprintln!("Sending UDP datagrams to {addr_str}");
    }
    if let Some(log) = &opts.instrumentation.json_log {
        log.log_connect(&addr_str);
    }

    let result = relay(socket, &opts.instrumentation, opts.filter);

    if let Some(log) = &opts.instrumentation.json_log {
        log.log_disconnect(&addr_str);
    }
    if let Some(stats) = &opts.instrumentation.stats {
        stats.print_summary();
    }

    result
}

pub fn run_server(opts: &ServerOptions) -> io::Result<()> {
    let socket = UdpSocket::bind((opts.bind_addr, opts.port))?;
    if opts.verbose > 0 {
        eprintln!("Listening for UDP datagrams on {}:{}", opts.bind_addr, opts.port);
    }

    // Learn the peer from the first datagram received, then "connect" the
    // socket so subsequent send()/recv() calls only talk to that peer.
    let mut buf = [0u8; BUF_SIZE];
    let (n, peer) = socket.recv_from(&mut buf)?;
    socket.connect(peer)?;
    if opts.verbose > 0 {
        eprintln!("Datagram received from {peer}");
    }
    if let Some(log) = &opts.instrumentation.json_log {
        log.log_connect(&peer.to_string());
    }
    opts.instrumentation.record(Direction::Recv, &buf[..n]);
    io::stdout().write_all(&buf[..n])?;
    io::stdout().flush()?;

    if let Some(secs) = opts.timeout {
        socket.set_read_timeout(Some(Duration::from_secs(secs)))?;
    }

    let result = relay(socket, &opts.instrumentation, opts.filter);

    if let Some(log) = &opts.instrumentation.json_log {
        log.log_disconnect(&peer.to_string());
    }
    if let Some(stats) = &opts.instrumentation.stats {
        stats.print_summary();
    }

    result
}

/// Relay data between stdin/stdout and a "connected" UDP socket, in both
/// directions at once. Each stdin read becomes one outgoing datagram.
fn relay(socket: UdpSocket, instrumentation: &Instrumentation, filter: Option<&Regex>) -> io::Result<()> {
    let send_socket = socket.try_clone()?;
    let send_instrumentation = instrumentation.clone();

    let writer = thread::spawn(move || -> io::Result<()> {
        let mut stdin = io::stdin();
        let mut buf = [0u8; BUF_SIZE];
        loop {
            let n = stdin.read(&mut buf)?;
            if n == 0 {
                break;
            }
            send_instrumentation.record(Direction::Send, &buf[..n]);
            send_socket.send(&buf[..n])?;
        }
        Ok(())
    });

    let mut line_filter = filter.map(|re| LineFilter::new(re.clone()));
    let mut stdout = io::stdout();
    let mut buf = [0u8; BUF_SIZE];
    loop {
        match socket.recv(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                instrumentation.record(Direction::Recv, &buf[..n]);
                match &mut line_filter {
                    Some(f) => f.feed(&buf[..n], &mut stdout)?,
                    None => stdout.write_all(&buf[..n])?,
                }
                stdout.flush()?;
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
    }
    if let Some(mut f) = line_filter {
        f.flush_remainder(&mut stdout)?;
    }

    match writer.join().unwrap_or(Ok(())) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => Ok(()),
        Err(e) => Err(e),
    }
}

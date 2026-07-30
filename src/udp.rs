use std::io::{self, Read, Write};
use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

const BUF_SIZE: usize = 65536;

/// Build a "host:port" string that also works for bare IPv6 literals.
fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn run_client(host: &str, port: u16, timeout: Option<u64>, verbose: u8) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let addr_str = format_addr(host, port);
    socket.connect(&addr_str)?;

    if let Some(secs) = timeout {
        socket.set_read_timeout(Some(Duration::from_secs(secs)))?;
    }

    if verbose > 0 {
        eprintln!("Sending UDP datagrams to {addr_str}");
    }

    relay(socket)
}

pub fn run_server(bind_addr: &str, port: u16, timeout: Option<u64>, verbose: u8) -> io::Result<()> {
    let socket = UdpSocket::bind((bind_addr, port))?;
    if verbose > 0 {
        eprintln!("Listening for UDP datagrams on {bind_addr}:{port}");
    }

    // Learn the peer from the first datagram received, then "connect" the
    // socket so subsequent send()/recv() calls only talk to that peer.
    let mut buf = [0u8; BUF_SIZE];
    let (n, peer) = socket.recv_from(&mut buf)?;
    socket.connect(peer)?;
    if verbose > 0 {
        eprintln!("Datagram received from {peer}");
    }
    io::stdout().write_all(&buf[..n])?;
    io::stdout().flush()?;

    if let Some(secs) = timeout {
        socket.set_read_timeout(Some(Duration::from_secs(secs)))?;
    }

    relay(socket)
}

/// Relay data between stdin/stdout and a "connected" UDP socket, in both
/// directions at once. Each stdin read becomes one outgoing datagram.
fn relay(socket: UdpSocket) -> io::Result<()> {
    let send_socket = socket.try_clone()?;

    let writer = thread::spawn(move || -> io::Result<()> {
        let mut stdin = io::stdin();
        let mut buf = [0u8; BUF_SIZE];
        loop {
            let n = stdin.read(&mut buf)?;
            if n == 0 {
                break;
            }
            send_socket.send(&buf[..n])?;
        }
        Ok(())
    });

    let mut stdout = io::stdout();
    let mut buf = [0u8; BUF_SIZE];
    loop {
        match socket.recv(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                stdout.write_all(&buf[..n])?;
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

    let _ = writer.join();
    Ok(())
}

use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

/// Build a "host:port" string that also works for bare IPv6 literals.
fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn run_client(
    host: &str,
    port: u16,
    timeout: Option<u64>,
    exec: Option<&str>,
    verbose: u8,
) -> io::Result<()> {
    let addr_str = format_addr(host, port);
    let stream = if let Some(secs) = timeout {
        let sockaddr = addr_str
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not resolve address"))?;
        TcpStream::connect_timeout(&sockaddr, Duration::from_secs(secs))?
    } else {
        TcpStream::connect(&addr_str)?
    };

    if verbose > 0 {
        eprintln!(
            "Connected to {} ({})",
            addr_str,
            stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()
        );
    }

    if let Some(secs) = timeout {
        stream.set_read_timeout(Some(Duration::from_secs(secs)))?;
    }

    handle_connection(stream, exec, verbose)
}

pub fn run_server(
    bind_addr: &str,
    port: u16,
    keep_open: bool,
    exec: Option<&str>,
    verbose: u8,
) -> io::Result<()> {
    let listener = TcpListener::bind((bind_addr, port))?;
    if verbose > 0 {
        eprintln!("Listening on {bind_addr}:{port}");
    }

    loop {
        let (stream, peer) = listener.accept()?;
        if verbose > 0 {
            eprintln!("Connection received from {peer}");
        }

        if let Err(e) = handle_connection(stream, exec, verbose) {
            eprintln!("connection error: {e}");
        }

        if verbose > 0 {
            eprintln!("Connection from {peer} closed");
        }

        if !keep_open {
            break;
        }
    }

    Ok(())
}

fn handle_connection(stream: TcpStream, exec: Option<&str>, verbose: u8) -> io::Result<()> {
    match exec {
        Some(cmd) => exec_over_stream(stream, cmd, verbose),
        None => relay_stdio(stream),
    }
}

/// Relay data between stdin/stdout and the socket, in both directions at once.
fn relay_stdio(stream: TcpStream) -> io::Result<()> {
    let mut sock_in = stream.try_clone()?;
    let mut sock_out = stream;

    let writer = thread::spawn(move || -> io::Result<()> {
        let mut stdin = io::stdin();
        io::copy(&mut stdin, &mut sock_in)?;
        let _ = sock_in.shutdown(Shutdown::Write);
        Ok(())
    });

    let mut stdout = io::stdout();
    let _ = io::copy(&mut sock_out, &mut stdout);
    let _ = stdout.flush();
    let _ = sock_out.shutdown(Shutdown::Read);

    let _ = writer.join();
    Ok(())
}

/// Spawn `cmd` in a shell and wire its stdin/stdout/stderr to the socket.
///
/// This is netcat's classic `-e` behavior. It hands a shell to whoever is on
/// the other end of the connection, so only use it against systems and
/// networks you're authorized to test (CTF boxes, your own lab, an
/// authorized pentest engagement).
fn exec_over_stream(stream: TcpStream, cmd: &str, verbose: u8) -> io::Result<()> {
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

    let mut sock_in = stream.try_clone()?;
    let mut sock_out = stream.try_clone()?;
    let mut sock_err = stream;

    let t_in = thread::spawn(move || {
        let _ = io::copy(&mut sock_in, &mut child_stdin);
    });
    let t_out = thread::spawn(move || {
        let _ = io::copy(&mut child_stdout, &mut sock_out);
    });
    let t_err = thread::spawn(move || {
        let _ = io::copy(&mut child_stderr, &mut sock_err);
    });

    let _ = child.wait();
    let _ = t_in.join();
    let _ = t_out.join();
    let _ = t_err.join();

    Ok(())
}

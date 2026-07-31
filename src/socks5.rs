use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Minimal SOCKS5 (RFC 1928) client: connects to `proxy_addr`, performs the
/// no-authentication handshake, and issues a CONNECT for `target_host:target_port`.
/// Returns the resulting stream, through which bytes flow exactly as if we
/// had dialed the target directly.
pub fn connect_via_proxy(
    proxy_addr: &str,
    target_host: &str,
    target_port: u16,
    timeout: Option<Duration>,
) -> io::Result<TcpStream> {
    let mut stream = match timeout {
        Some(t) => {
            let addr = proxy_addr
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not resolve proxy address"))?;
            TcpStream::connect_timeout(&addr, t)?
        }
        None => TcpStream::connect(proxy_addr)?,
    };

    // Greeting: version 5, 1 auth method offered, "no authentication".
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp)?;
    if resp[0] != 0x05 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "proxy did not speak SOCKS5",
        ));
    }
    if resp[1] != 0x00 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SOCKS5 proxy requires authentication, which is not supported",
        ));
    }

    // CONNECT request, target given as a domain name (ATYP 0x03) so the
    // proxy performs its own DNS resolution.
    let host_bytes = target_host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination hostname too long for SOCKS5",
        ));
    }
    let mut req = Vec::with_capacity(7 + host_bytes.len());
    req.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&req)?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head)?;
    if head[0] != 0x05 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed SOCKS5 reply",
        ));
    }
    if head[1] != 0x00 {
        return Err(io::Error::other(describe_reply(head[1])));
    }

    // Consume BND.ADDR + BND.PORT so the connection is left in a clean state.
    match head[3] {
        0x01 => skip(&mut stream, 4 + 2)?,
        0x04 => skip(&mut stream, 16 + 2)?,
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            skip(&mut stream, len[0] as usize + 2)?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown SOCKS5 address type in reply",
            ))
        }
    }

    Ok(stream)
}

fn skip(stream: &mut TcpStream, n: usize) -> io::Result<()> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)
}

fn describe_reply(code: u8) -> String {
    let reason = match code {
        0x01 => "general SOCKS server failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    };
    format!("SOCKS5 proxy rejected the request: {reason}")
}

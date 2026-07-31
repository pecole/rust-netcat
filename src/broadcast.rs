use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::instrument::{Direction, Instrumentation};

type Clients = Arc<Mutex<HashMap<SocketAddr, TcpStream>>>;

/// A netcat feature nc doesn't have: instead of relaying stdio<->socket for
/// a single peer, keep accepting connections and relay every message a
/// client sends to every *other* connected client - a minimal chat room /
/// message bus, with no participation from this process's own stdio.
pub fn run(bind_addr: &str, port: u16, verbose: u8, instrumentation: Instrumentation) -> io::Result<()> {
    let listener = TcpListener::bind((bind_addr, port))?;
    if verbose > 0 {
        eprintln!("Listening on {bind_addr}:{port} (broadcast mode)");
    }

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let peer = stream.peer_addr()?;
        if verbose > 0 {
            eprintln!("Client connected: {peer}");
        }
        if let Some(log) = &instrumentation.json_log {
            log.log_connect(&peer.to_string());
        }

        let write_handle = stream.try_clone()?;
        clients.lock().expect("client map poisoned").insert(peer, write_handle);

        let clients = Arc::clone(&clients);
        let instrumentation = instrumentation.clone();
        thread::spawn(move || handle_client(stream, peer, clients, instrumentation, verbose));
    }

    Ok(())
}

fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    clients: Clients,
    instrumentation: Instrumentation,
    verbose: u8,
) {
    let mut buf = [0u8; 8192];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };

        instrumentation.record(Direction::Recv, &buf[..n]);

        let mut targets = clients.lock().expect("client map poisoned");
        targets.retain(|&addr, writer| addr == peer || writer.write_all(&buf[..n]).is_ok());
    }

    // Don't remove `peer`'s write handle here: a half-closed stdin (EOF)
    // only means this client is done *sending*, not that it stopped
    // listening. Leave it in `clients` so it still receives broadcasts;
    // the `retain` above will prune it once a write to it actually fails
    // (i.e. once it's truly gone).
    if verbose > 0 {
        eprintln!("Client input closed: {peer}");
    }
    if let Some(log) = &instrumentation.json_log {
        log.log_disconnect(&peer.to_string());
    }
}

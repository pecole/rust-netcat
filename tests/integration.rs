use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn rnc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rnc"))
}

#[test]
fn tcp_client_to_server_roundtrip() {
    let port = free_tcp_port();

    let server = rnc()
        .args(["-l", "-p", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args(["-w", "2", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client");

    client
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello-integration-test")
        .unwrap();

    let client_status = client.wait().expect("client did not exit");
    assert!(client_status.success());

    let server_output = server.wait_with_output().expect("server did not exit");
    assert_eq!(server_output.stdout, b"hello-integration-test");
}

#[test]
fn udp_client_to_server_roundtrip() {
    let port = free_udp_port();

    let mut server = rnc()
        .args(["-u", "-l", "-w", "1", "-p", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args(["-u", "-w", "1", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client");

    client
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello-udp-test")
        .unwrap();

    let client_status = client.wait().expect("client did not exit");
    assert!(client_status.success());

    let server_status = server.wait().expect("server did not exit");
    assert!(server_status.success());

    let mut output = Vec::new();
    server
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert_eq!(output, b"hello-udp-test");
}

#[test]
fn keep_open_accepts_a_second_connection() {
    let port = free_tcp_port();

    let mut server = rnc()
        .args(["-l", "-k", "-p", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    for msg in ["first", "second"] {
        let mut client = rnc()
            .args(["-w", "2", "127.0.0.1", &port.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn client");
        client
            .stdin
            .take()
            .unwrap()
            .write_all(msg.as_bytes())
            .unwrap();
        let status = client.wait().expect("client did not exit");
        assert!(status.success());
        thread::sleep(Duration::from_millis(200));
    }

    server.kill().expect("failed to kill server");
    let output = server.wait_with_output().expect("server did not exit");
    assert_eq!(output.stdout, b"firstsecond");
}

#[test]
fn hex_dump_and_stats_appear_on_stderr() {
    let port = free_tcp_port();

    let server = rnc()
        .args(["-l", "-p", &port.to_string(), "-x", "--stats"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args(["-w", "2", "127.0.0.1", &port.to_string(), "-x", "--stats"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn client");
    client.stdin.take().unwrap().write_all(b"Hi").unwrap();
    let client_output = client.wait_with_output().expect("client did not exit");
    assert!(client_output.status.success());

    let server_output = server.wait_with_output().expect("server did not exit");

    let client_stderr = String::from_utf8_lossy(&client_output.stderr);
    assert!(client_stderr.contains(">> 2 bytes"), "missing hex dump: {client_stderr}");
    assert!(client_stderr.contains("48 69"), "missing hex bytes for 'Hi': {client_stderr}");
    assert!(client_stderr.contains("--- stats:"), "missing stats summary: {client_stderr}");

    let server_stderr = String::from_utf8_lossy(&server_output.stderr);
    assert!(server_stderr.contains("<< 2 bytes"), "missing hex dump: {server_stderr}");
    assert!(server_stderr.contains("--- stats:"), "missing stats summary: {server_stderr}");
}

#[test]
fn filter_only_forwards_matching_lines() {
    let port = free_tcp_port();

    let server = rnc()
        .args(["-l", "-p", &port.to_string(), "--filter", "ERROR"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args(["-w", "1", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client");
    client
        .stdin
        .take()
        .unwrap()
        .write_all(b"normal line\nERROR: bad thing\nanother normal line\n")
        .unwrap();
    let status = client.wait().expect("client did not exit");
    assert!(status.success());

    let output = server.wait_with_output().expect("server did not exit");
    assert_eq!(output.stdout, b"ERROR: bad thing\n");
}

#[test]
fn json_log_records_connect_and_data_events() {
    let port = free_tcp_port();
    let log_path = std::env::temp_dir().join(format!("rnc_test_jsonlog_{port}.jsonl"));
    let _ = fs::remove_file(&log_path);

    let server = rnc()
        .args(["-l", "-p", &port.to_string(), "--json-log", log_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args(["-w", "1", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client");
    client.stdin.take().unwrap().write_all(b"logged").unwrap();
    let status = client.wait().expect("client did not exit");
    assert!(status.success());

    server.wait_with_output().expect("server did not exit");

    let log = fs::read_to_string(&log_path).expect("json log file should exist");
    assert!(log.contains(r#""event":"connect""#), "missing connect event: {log}");
    assert!(log.contains(r#""event":"data""#), "missing data event: {log}");
    assert!(log.contains(r#""event":"disconnect""#), "missing disconnect event: {log}");

    let _ = fs::remove_file(&log_path);
}

#[test]
fn retry_eventually_connects_once_server_starts() {
    let port = free_tcp_port();

    // No server listening yet: the client must retry until one appears.
    let mut client = rnc()
        .args([
            "--retry",
            "5",
            "--retry-delay",
            "1",
            "-w",
            "2",
            "127.0.0.1",
            &port.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn client");
    client.stdin.take().unwrap().write_all(b"retried").unwrap();

    thread::sleep(Duration::from_millis(1200));
    let server = rnc()
        .args(["-l", "-p", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    let client_output = client.wait_with_output().expect("client did not exit");
    assert!(
        client_output.status.success(),
        "client should eventually succeed: {}",
        String::from_utf8_lossy(&client_output.stderr)
    );

    let server_output = server.wait_with_output().expect("server did not exit");
    assert_eq!(server_output.stdout, b"retried");
}

#[test]
fn broadcast_relays_between_clients() {
    let port = free_tcp_port();

    let mut server = rnc()
        .args(["-l", "-k", "-p", &port.to_string(), "--broadcast"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    // Client A: keeps its stdin pipe open (not immediately EOF) so its
    // write-side half-close doesn't drop it from the broadcast roster
    // before B's message arrives.
    let mut client_a = rnc()
        .args(["-w", "2", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client A");
    let client_a_stdin = client_a.stdin.take().unwrap();

    thread::sleep(Duration::from_millis(300));

    let mut client_b = rnc()
        .args(["-w", "1", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client B");
    client_b
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello from B")
        .unwrap();
    let b_status = client_b.wait().expect("client B did not exit");
    assert!(b_status.success());

    // Now let A's stdin hit EOF so its own relay finishes within -w.
    drop(client_a_stdin);
    let a_output = client_a.wait_with_output().expect("client A did not exit");
    assert!(a_output.status.success());
    assert_eq!(a_output.stdout, b"hello from B");

    server.kill().expect("failed to kill server");
    let _ = server.wait();
}

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Generates a self-signed leaf certificate (CA:FALSE, proper SAN for
/// 127.0.0.1/localhost) suitable for TLS server auth, keyed off `port` so
/// concurrently-running tests don't clobber each other's files.
fn generate_self_signed_cert(port: u16) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir();
    let cert = dir.join(format!("rnc_test_cert_{port}.pem"));
    let key = dir.join(format!("rnc_test_key_{port}.pem"));

    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            key.to_str().unwrap(),
            "-out",
            cert.to_str().unwrap(),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-addext",
            "keyUsage=critical,digitalSignature,keyEncipherment",
            "-addext",
            "extendedKeyUsage=serverAuth",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed to run openssl");
    assert!(status.success(), "openssl cert generation failed");

    (cert, key)
}

#[test]
fn tls_round_trip_with_insecure_client() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let port = free_tcp_port();
    let (cert, key) = generate_self_signed_cert(port);

    let server = rnc()
        .args([
            "-l",
            "-p",
            &port.to_string(),
            "--tls",
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn TLS server");

    thread::sleep(Duration::from_millis(400));

    let mut client = rnc()
        .args(["-w", "2", "--tls", "--tls-insecure", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn TLS client");
    client.stdin.take().unwrap().write_all(b"over-tls").unwrap();
    let client_output = client.wait_with_output().expect("client did not exit");
    assert!(
        client_output.status.success(),
        "TLS client failed: {}",
        String::from_utf8_lossy(&client_output.stderr)
    );

    let server_output = server.wait_with_output().expect("server did not exit");
    assert_eq!(server_output.stdout, b"over-tls");

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
}

#[test]
fn tls_rejects_untrusted_certificate() {
    if !openssl_available() {
        eprintln!("skipping: openssl not available");
        return;
    }
    let port = free_tcp_port();
    let (cert, key) = generate_self_signed_cert(port);

    let server = rnc()
        .args([
            "-l",
            "-p",
            &port.to_string(),
            "--tls",
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn TLS server");

    thread::sleep(Duration::from_millis(400));

    // No --tls-insecure and no --tls-ca: the self-signed cert must be rejected.
    let mut client = rnc()
        .args(["-w", "2", "--tls", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn TLS client");
    client.stdin.take().unwrap().write_all(b"should-not-arrive").unwrap();
    let client_output = client.wait_with_output().expect("client did not exit");
    assert!(!client_output.status.success(), "client should reject an untrusted cert");
    assert!(String::from_utf8_lossy(&client_output.stderr).contains("certificate"));

    let server_output = server.wait_with_output().expect("server did not exit");
    assert_eq!(server_output.stdout, b"");

    let _ = fs::remove_file(&cert);
    let _ = fs::remove_file(&key);
}

/// A minimal single-connection SOCKS5 CONNECT server (RFC 1928, no-auth
/// only), just enough to exercise `rnc --proxy` without depending on a
/// real proxy binary being installed.
fn spawn_test_socks5_proxy() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        let (mut client, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => return,
        };

        let mut greeting = [0u8; 2];
        if client.read_exact(&mut greeting).is_err() {
            return;
        }
        let mut methods = vec![0u8; greeting[1] as usize];
        if client.read_exact(&mut methods).is_err() {
            return;
        }
        if client.write_all(&[0x05, 0x00]).is_err() {
            return;
        }

        let mut head = [0u8; 4];
        if client.read_exact(&mut head).is_err() {
            return;
        }
        let host = match head[3] {
            0x03 => {
                let mut len = [0u8; 1];
                if client.read_exact(&mut len).is_err() {
                    return;
                }
                let mut host_buf = vec![0u8; len[0] as usize];
                if client.read_exact(&mut host_buf).is_err() {
                    return;
                }
                String::from_utf8_lossy(&host_buf).into_owned()
            }
            _ => return,
        };
        let mut port_buf = [0u8; 2];
        if client.read_exact(&mut port_buf).is_err() {
            return;
        }
        let target_port = u16::from_be_bytes(port_buf);

        let target = match TcpStream::connect((host.as_str(), target_port)) {
            Ok(t) => t,
            Err(_) => {
                let _ = client.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
                return;
            }
        };
        if client
            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .is_err()
        {
            return;
        }

        let mut client_read = client.try_clone().unwrap();
        let mut target_write = target.try_clone().unwrap();
        let mut target_read = target;
        let mut client_write = client;
        let t1 = thread::spawn(move || {
            let _ = std::io::copy(&mut client_read, &mut target_write);
        });
        let _ = std::io::copy(&mut target_read, &mut client_write);
        let _ = t1.join();
    });

    port
}

#[test]
fn socks5_proxy_connects_through_proxy() {
    let proxy_port = spawn_test_socks5_proxy();
    thread::sleep(Duration::from_millis(200));

    let target_port = free_tcp_port();
    let server = rnc()
        .args(["-l", "-p", &target_port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn target server");

    thread::sleep(Duration::from_millis(300));

    let mut client = rnc()
        .args([
            "-w",
            "2",
            "--proxy",
            &format!("127.0.0.1:{proxy_port}"),
            "127.0.0.1",
            &target_port.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn proxied client");
    client.stdin.take().unwrap().write_all(b"via-proxy").unwrap();
    let client_output = client.wait_with_output().expect("client did not exit");
    assert!(
        client_output.status.success(),
        "proxied client failed: {}",
        String::from_utf8_lossy(&client_output.stderr)
    );

    let server_output = server.wait_with_output().expect("server did not exit");
    assert_eq!(server_output.stdout, b"via-proxy");
}

#[test]
fn rate_limit_slows_down_a_transfer() {
    let port = free_tcp_port();
    let payload = vec![b'x'; 20_000];

    let server = rnc()
        .args(["-l", "-p", &port.to_string(), "--rate-limit", "10000"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    thread::sleep(Duration::from_millis(300));

    let start = Instant::now();
    let mut client = rnc()
        .args(["-w", "5", "--rate-limit", "10000", "127.0.0.1", &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn client");
    client.stdin.take().unwrap().write_all(&payload).unwrap();
    let status = client.wait().expect("client did not exit");
    assert!(status.success());
    let elapsed = start.elapsed();

    // 20,000 bytes at 10,000 B/s should take ~2s; a completely unthrottled
    // loopback transfer would take milliseconds, so this bounds out flukes
    // without being sensitive to exact scheduling.
    assert!(
        elapsed >= Duration::from_millis(1500),
        "transfer finished too fast for the configured rate limit: {elapsed:?}"
    );

    let output = server.wait_with_output().expect("server did not exit");
    assert_eq!(output.stdout, payload);
}

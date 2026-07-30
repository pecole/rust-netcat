use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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

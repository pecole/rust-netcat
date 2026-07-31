use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConnection, DigitallySignedStruct, ServerConnection, SignatureScheme};

use crate::stream::{BoxedStream, NetStream};

/// How often the underlying socket read unblocks so the write-side thread
/// gets a chance to take the lock. Purely an implementation detail of
/// running one TLS session from two threads; unrelated to `-w`.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn io_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

#[derive(Debug)]
struct NoServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn client_config(insecure: bool, ca_path: Option<&Path>) -> io::Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder();

    let config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoServerVerification(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = ca_path {
            let mut reader = BufReader::new(File::open(path)?);
            for cert in rustls_pemfile::certs(&mut reader) {
                roots.add(cert.map_err(io_err)?).map_err(io_err)?;
            }
        } else {
            let native = rustls_native_certs::load_native_certs().map_err(io_err)?;
            for cert in native {
                let _ = roots.add(cert);
            }
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };

    Ok(Arc::new(config))
}

pub fn server_config(cert_path: &Path, key_path: &Path) -> io::Result<Arc<rustls::ServerConfig>> {
    let certs = rustls_pemfile::certs(&mut BufReader::new(File::open(cert_path)?))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err)?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(File::open(key_path)?))
        .map_err(io_err)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no private key found in --tls-key file"))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(io_err)?;

    Ok(Arc::new(config))
}

enum TlsConn {
    Client(rustls::StreamOwned<ClientConnection, TcpStream>),
    Server(rustls::StreamOwned<ServerConnection, TcpStream>),
}

impl TlsConn {
    fn close_notify(&mut self) {
        match self {
            TlsConn::Client(s) => {
                s.conn.send_close_notify();
                let _ = s.flush();
            }
            TlsConn::Server(s) => {
                s.conn.send_close_notify();
                let _ = s.flush();
            }
        }
    }
}

impl Read for TlsConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            TlsConn::Client(s) => s.read(buf),
            TlsConn::Server(s) => s.read(buf),
        }
    }
}

impl Write for TlsConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            TlsConn::Client(s) => s.write(buf),
            TlsConn::Server(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            TlsConn::Client(s) => s.flush(),
            TlsConn::Server(s) => s.flush(),
        }
    }
}

/// One handle onto a shared TLS session. Two of these (cloned from the same
/// `Arc<Mutex<..>>`) let one thread read while another writes; each
/// operation just takes the lock for the duration of that single call.
pub struct TlsHalf {
    inner: Arc<Mutex<TlsConn>>,
    idle_timeout: Option<Duration>,
}

impl Read for TlsHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let idle_start = Instant::now();
        loop {
            let result = {
                let mut conn = self.inner.lock().expect("tls lock poisoned");
                conn.read(buf)
            };
            match result {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                    if let Some(timeout) = self.idle_timeout {
                        if idle_start.elapsed() >= timeout {
                            return Err(io::Error::new(io::ErrorKind::TimedOut, "read timed out"));
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Write for TlsHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().expect("tls lock poisoned").write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().expect("tls lock poisoned").flush()
    }
}

impl NetStream for TlsHalf {
    fn shutdown_write(&self) -> io::Result<()> {
        if let Ok(mut conn) = self.inner.lock() {
            conn.close_notify();
        }
        Ok(())
    }

    fn try_clone_boxed(&self) -> io::Result<BoxedStream> {
        Ok(Box::new(TlsHalf {
            inner: Arc::clone(&self.inner),
            idle_timeout: self.idle_timeout,
        }))
    }
}

fn wrap(conn: TlsConn, idle_timeout: Option<Duration>) -> BoxedStream {
    Box::new(TlsHalf {
        inner: Arc::new(Mutex::new(conn)),
        idle_timeout,
    })
}

pub fn wrap_client(
    stream: TcpStream,
    server_name: &str,
    config: Arc<rustls::ClientConfig>,
    idle_timeout: Option<Duration>,
) -> io::Result<BoxedStream> {
    stream.set_read_timeout(Some(POLL_INTERVAL))?;
    let name = ServerName::try_from(server_name.to_string())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let conn = ClientConnection::new(config, name).map_err(io_err)?;
    Ok(wrap(TlsConn::Client(rustls::StreamOwned::new(conn, stream)), idle_timeout))
}

pub fn wrap_server(
    stream: TcpStream,
    config: Arc<rustls::ServerConfig>,
    idle_timeout: Option<Duration>,
) -> io::Result<BoxedStream> {
    stream.set_read_timeout(Some(POLL_INTERVAL))?;
    let conn = ServerConnection::new(config).map_err(io_err)?;
    Ok(wrap(TlsConn::Server(rustls::StreamOwned::new(conn, stream)), idle_timeout))
}

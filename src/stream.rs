use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};

/// A duplex network stream. `shutdown_write` lets a client-side "half-close"
/// after stdin hits EOF so the peer sees the end of our data without us
/// having to tear down the whole connection; implementations that can't
/// express a half-close (TLS) fall back to a no-op. `try_clone_boxed` hands
/// out another independent handle onto the same connection so a caller can
/// drive reads and writes from separate threads (or, for `-e`, wire up
/// stdin/stdout/stderr of a child process each on their own handle).
pub trait NetStream: Read + Write {
    fn shutdown_write(&self) -> io::Result<()> {
        Ok(())
    }

    fn try_clone_boxed(&self) -> io::Result<BoxedStream>;
}

pub type BoxedStream = Box<dyn NetStream + Send>;

impl NetStream for TcpStream {
    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }

    fn try_clone_boxed(&self) -> io::Result<BoxedStream> {
        Ok(Box::new(self.try_clone()?))
    }
}

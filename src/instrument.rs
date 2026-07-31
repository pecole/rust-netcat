use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Data flowing from us to the peer (stdin -> socket).
    Send,
    /// Data flowing from the peer to us (socket -> stdout).
    Recv,
}

impl Direction {
    fn arrow(self) -> &'static str {
        match self {
            Direction::Send => ">>",
            Direction::Recv => "<<",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Direction::Send => "send",
            Direction::Recv => "recv",
        }
    }
}

/// Transfer counters, printed as a one-line summary when the connection ends.
pub struct Stats {
    bytes_sent: AtomicU64,
    bytes_recv: AtomicU64,
    started: Instant,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            bytes_sent: AtomicU64::new(0),
            bytes_recv: AtomicU64::new(0),
            started: Instant::now(),
        }
    }

    fn add(&self, dir: Direction, n: u64) {
        let counter = match dir {
            Direction::Send => &self.bytes_sent,
            Direction::Recv => &self.bytes_recv,
        };
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub fn print_summary(&self) {
        let elapsed = self.started.elapsed().as_secs_f64().max(1e-6);
        let sent = self.bytes_sent.load(Ordering::Relaxed);
        let recv = self.bytes_recv.load(Ordering::Relaxed);
        eprintln!(
            "--- stats: sent {sent} bytes, received {recv} bytes, elapsed {elapsed:.2}s ({:.1} KiB/s tx, {:.1} KiB/s rx) ---",
            sent as f64 / 1024.0 / elapsed,
            recv as f64 / 1024.0 / elapsed,
        );
    }
}

/// Caps combined throughput to roughly `bytes_per_sec` using a simple
/// fixed-window limiter: once a 1-second window's budget is spent, callers
/// block until the window rolls over.
pub struct RateLimiter {
    limit: u64,
    start: Instant,
    sent: AtomicU64,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            limit: bytes_per_sec,
            start: Instant::now(),
            sent: AtomicU64::new(0),
        }
    }

    /// Leaky-bucket pacing: track total bytes moved since the limiter was
    /// created, work out how long that should have taken at the target
    /// rate, and sleep off the difference if we're running ahead of
    /// schedule. Unlike a fixed-window counter, this scales smoothly
    /// regardless of how large each chunk is relative to the limit.
    pub fn throttle(&self, n: u64) {
        if self.limit == 0 {
            return;
        }
        let total = self.sent.fetch_add(n, Ordering::Relaxed) + n;
        let expected = Duration::from_secs_f64(total as f64 / self.limit as f64);
        let actual = self.start.elapsed();
        if expected > actual {
            thread_sleep(expected - actual);
        }
    }
}

fn thread_sleep(d: Duration) {
    std::thread::sleep(d);
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Structured JSON-lines event log: one connect/disconnect/data/error event
/// per line, so the log can be tailed or fed into `jq` while the connection
/// is live.
pub struct JsonLogger {
    file: Mutex<std::fs::File>,
}

impl JsonLogger {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    fn write_line(&self, line: &str) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn log_connect(&self, peer: &str) {
        self.write_line(&format!(
            r#"{{"ts":{},"event":"connect","peer":"{}"}}"#,
            epoch_millis(),
            json_escape(peer)
        ));
    }

    pub fn log_disconnect(&self, peer: &str) {
        self.write_line(&format!(
            r#"{{"ts":{},"event":"disconnect","peer":"{}"}}"#,
            epoch_millis(),
            json_escape(peer)
        ));
    }

    pub fn log_data(&self, dir: Direction, n: usize) {
        self.write_line(&format!(
            r#"{{"ts":{},"event":"data","direction":"{}","bytes":{}}}"#,
            epoch_millis(),
            dir.label(),
            n
        ));
    }

    pub fn log_error(&self, msg: &str) {
        self.write_line(&format!(
            r#"{{"ts":{},"event":"error","message":"{}"}}"#,
            epoch_millis(),
            json_escape(msg)
        ));
    }
}

fn print_hexdump(dir: Direction, buf: &[u8], timestamps: bool, start: Instant) {
    let mut out = String::new();
    if timestamps {
        out.push_str(&format!("[{:>8.3}s] ", start.elapsed().as_secs_f64()));
    }
    out.push_str(&format!("{} {} bytes\n", dir.arrow(), buf.len()));
    for chunk in buf.chunks(16) {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("    {:<47}  {}\n", hex.join(" "), ascii));
    }
    eprint!("{out}");
}

/// Line-buffered regex filter for the receive leg: only complete lines
/// matching the pattern are forwarded to the real output. Used for
/// `--filter`, which is a display filter (it never affects what a `-e`
/// child process or the wire itself sees).
pub struct LineFilter {
    regex: Regex,
    buffer: Vec<u8>,
}

impl LineFilter {
    pub fn new(regex: Regex) -> Self {
        Self {
            regex,
            buffer: Vec::new(),
        }
    }

    pub fn feed<W: Write + ?Sized>(&mut self, data: &[u8], out: &mut W) -> io::Result<()> {
        self.buffer.extend_from_slice(data);
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            if self.regex.is_match(&String::from_utf8_lossy(&line)) {
                out.write_all(&line)?;
            }
        }
        Ok(())
    }

    pub fn flush_remainder<W: Write + ?Sized>(&mut self, out: &mut W) -> io::Result<()> {
        if !self.buffer.is_empty() && self.regex.is_match(&String::from_utf8_lossy(&self.buffer)) {
            out.write_all(&self.buffer)?;
        }
        self.buffer.clear();
        Ok(())
    }
}

/// Bundles all of the optional observability/shaping features so call sites
/// only need to thread one value through instead of five.
#[derive(Clone)]
pub struct Instrumentation {
    pub hex: bool,
    pub timestamps: bool,
    pub stats: Option<std::sync::Arc<Stats>>,
    pub json_log: Option<std::sync::Arc<JsonLogger>>,
    pub rate_limiter: Option<std::sync::Arc<RateLimiter>>,
    start: Option<Instant>,
}

impl Instrumentation {
    pub fn configured(
        hex: bool,
        timestamps: bool,
        stats: Option<std::sync::Arc<Stats>>,
        json_log: Option<std::sync::Arc<JsonLogger>>,
        rate_limiter: Option<std::sync::Arc<RateLimiter>>,
    ) -> Self {
        Self {
            hex,
            timestamps,
            stats,
            json_log,
            rate_limiter,
            start: Some(Instant::now()),
        }
    }

    pub fn record(&self, dir: Direction, buf: &[u8]) {
        if let Some(rl) = &self.rate_limiter {
            rl.throttle(buf.len() as u64);
        }
        if let Some(stats) = &self.stats {
            stats.add(dir, buf.len() as u64);
        }
        if self.hex {
            print_hexdump(dir, buf, self.timestamps, self.start.unwrap_or_else(Instant::now));
        }
        if let Some(log) = &self.json_log {
            log.log_data(dir, buf.len());
        }
    }
}

/// Copies bytes from `reader` to `writer` until EOF, running each chunk
/// through `instrumentation` (stats/hexdump/json-log/rate-limit) and,
/// optionally, a receive-side line filter. Returns the total bytes copied.
pub fn pump<R: io::Read + ?Sized, W: Write + ?Sized>(
    reader: &mut R,
    writer: &mut W,
    dir: Direction,
    instrumentation: &Instrumentation,
    filter: Option<&Mutex<LineFilter>>,
) -> io::Result<u64> {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        instrumentation.record(dir, &buf[..n]);
        match filter {
            Some(f) => f
                .lock()
                .expect("line filter lock poisoned")
                .feed(&buf[..n], writer)?,
            None => writer.write_all(&buf[..n])?,
        }
        // Flush promptly rather than batching: this pump is often driving
        // an interactive session (chat, a shell), where the peer needs to
        // see each chunk as it arrives, not once the whole stream ends.
        writer.flush()?;
        total += n as u64;
    }
    if let Some(f) = filter {
        f.lock()
            .expect("line filter lock poisoned")
            .flush_remainder(writer)?;
    }
    let _ = writer.flush();
    Ok(total)
}

//! What one client may hold of the server: connections, time and memory.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The server's limits. Each has a default (Puma's, where Puma has one) and
/// an environment variable that sets it, read by `Limits::from_env`.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Connections served at once (`MAX_CONNECTIONS`, 512). A connection
    /// past it gets a 503 and is closed, so slow clients can't take every
    /// thread, and at most this many requests queue for the workers. Each
    /// connection holds one file descriptor, so keep it under the process's
    /// descriptor limit, less one per worker and a few.
    pub max_connections: usize,
    /// How long a kept-alive connection may wait for its next request
    /// (`IDLE_TIMEOUT`, 20 s): Puma's `persistent_timeout`.
    pub idle_timeout: Duration,
    /// From a request's first byte to the end of its headers, however they
    /// trickle in (`HEADER_TIMEOUT`, 20 s). Past it, a 408.
    pub header_timeout: Duration,
    /// For a request's body (`BODY_TIMEOUT`, 60 s), plus a second for each
    /// `min_rate` bytes that arrive: a body of any size can take as long
    /// as it needs at that rate, and one that trickles in slower is a 408.
    pub body_timeout: Duration,
    /// The same for writing a response (`WRITE_TIMEOUT`, 60 s), to a
    /// client that reads it slowly or not at all.
    pub write_timeout: Duration,
    /// The slowest a body or a response may move on average once its
    /// timeout's grace is spent (`MIN_RATE`, 1024 bytes a second).
    pub min_rate: usize,
    /// The largest request body (`MAX_BODY_BYTES`, 10 MiB); a bigger one is
    /// a 413 before any of it is read.
    pub max_body_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 512,
            idle_timeout: Duration::from_secs(20),
            header_timeout: Duration::from_secs(20),
            body_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(60),
            min_rate: 1024,
            max_body_bytes: 10 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// The defaults, with any of `MAX_CONNECTIONS`, `IDLE_TIMEOUT`,
    /// `HEADER_TIMEOUT`, `BODY_TIMEOUT`, `WRITE_TIMEOUT` (seconds, fractions
    /// allowed, from a millisecond to 30 days), `MIN_RATE` and
    /// `MAX_BODY_BYTES` set in the environment.
    pub fn from_env() -> Result<Self, BoxError> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// `from_env` over any lookup.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, BoxError> {
        let defaults = Self::default();
        Ok(Self {
            max_connections: count(&var, "MAX_CONNECTIONS", defaults.max_connections)?,
            idle_timeout: seconds(&var, "IDLE_TIMEOUT", defaults.idle_timeout)?,
            header_timeout: seconds(&var, "HEADER_TIMEOUT", defaults.header_timeout)?,
            body_timeout: seconds(&var, "BODY_TIMEOUT", defaults.body_timeout)?,
            write_timeout: seconds(&var, "WRITE_TIMEOUT", defaults.write_timeout)?,
            min_rate: count(&var, "MIN_RATE", defaults.min_rate)?,
            max_body_bytes: count(&var, "MAX_BODY_BYTES", defaults.max_body_bytes)?,
        })
    }
}

fn count(var: &impl Fn(&str) -> Option<String>, name: &str, default: usize) -> Result<usize, BoxError> {
    let Some(text) = var(name) else { return Ok(default) };
    match text.trim().parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{name} must be a whole number above 0, not {text:?}").into()),
    }
}

/// A timeout between a millisecond and 30 days: shorter rounds to no
/// timeout at all in the socket calls, longer overflows a deadline.
fn seconds(var: &impl Fn(&str) -> Option<String>, name: &str, default: Duration) -> Result<Duration, BoxError> {
    let Some(text) = var(name) else { return Ok(default) };
    match text.trim().parse::<f64>().ok().filter(|s| (0.001..=30.0 * 86_400.0).contains(s)) {
        Some(seconds) => Ok(Duration::from_secs_f64(seconds)),
        None => Err(format!("{name} must be a number of seconds from 0.001 to 2592000, not {text:?}").into()),
    }
}

/// One direction of a socket, reads or writes, with a deadline the calls
/// share, so a client sending or reading a byte at a time can't hold the
/// connection past it. The deadline can grow with the bytes that move, at
/// a second per `rate` bytes. Without a deadline each call waits up to
/// `step`.
pub(crate) struct Timed<'a> {
    stream: &'a TcpStream,
    step: Duration,
    deadline: Option<Instant>,
    rate: Option<f64>,
}

impl<'a> Timed<'a> {
    pub(crate) fn new(stream: &'a TcpStream, step: Duration) -> Self {
        Self { stream, step, deadline: None, rate: None }
    }

    /// Everything from now on must be done within `limit`; None lifts it.
    pub(crate) fn within(&mut self, limit: Option<Duration>) {
        self.deadline = limit.and_then(|limit| Instant::now().checked_add(limit));
        self.rate = None;
    }

    /// Within `limit`, and a second more for each `rate` bytes that move.
    pub(crate) fn within_at(&mut self, limit: Duration, rate: usize) {
        self.within(Some(limit));
        self.rate = Some(rate as f64);
    }

    pub(crate) fn get_ref(&self) -> &TcpStream {
        self.stream
    }

    /// The next call's timeout, or TimedOut once the deadline has passed.
    /// Under a deadline a call may wait for all of what's left of it: a
    /// pause in the middle of a request is the deadline's to judge.
    fn timeout(&self) -> io::Result<Duration> {
        let Some(deadline) = self.deadline else { return Ok(self.step) };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { Err(io::ErrorKind::TimedOut.into()) } else { Ok(left) }
    }

    /// Bytes that moved extend a rate-based deadline.
    fn moved(&mut self, bytes: usize) {
        if let (Some(rate), Some(deadline)) = (self.rate, self.deadline) {
            self.deadline = deadline.checked_add(Duration::from_secs_f64(bytes as f64 / rate)).or(Some(deadline));
        }
    }

    /// A call cut short once the deadline has passed is the deadline's.
    fn expired(&self, error: io::Error) -> io::Error {
        let passed = self.deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let cut = matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut);
        if passed && cut { io::ErrorKind::TimedOut.into() } else { error }
    }
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.timeout()?))?;
        let read = { self.stream }.read(buf).map_err(|error| self.expired(error))?;
        self.moved(read);
        Ok(read)
    }
}

impl Write for Timed<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.timeout()?))?;
        let written = { self.stream }.write(buf).map_err(|error| self.expired(error))?;
        self.moved(written);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        { self.stream }.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn test_defaults_and_overrides() {
        assert_eq!(Limits::default(), Limits::from_vars(vars(&[])).unwrap());
        let set = Limits::from_vars(vars(&[("MAX_CONNECTIONS", "64"), ("HEADER_TIMEOUT", "2.5"), ("MAX_BODY_BYTES", "1024")])).unwrap();
        assert_eq!((64, Duration::from_millis(2500), 1024), (set.max_connections, set.header_timeout, set.max_body_bytes));
        assert_eq!(Limits::default().body_timeout, set.body_timeout);
    }

    #[test]
    fn test_a_value_that_is_not_a_limit_is_an_error() {
        let bad = [
            ("MAX_CONNECTIONS", "0"),
            ("MAX_CONNECTIONS", "lots"),
            ("MIN_RATE", "0"),
            ("IDLE_TIMEOUT", "-1"),
            ("IDLE_TIMEOUT", "1e-10"),
            ("HEADER_TIMEOUT", "9.3e18"),
            ("BODY_TIMEOUT", "NaN"),
            ("WRITE_TIMEOUT", "1e30"),
        ];
        for (name, value) in bad {
            let error = Limits::from_vars(vars(&[(name, value)])).unwrap_err();
            assert!(error.to_string().starts_with(name), "{error}");
        }
    }
}

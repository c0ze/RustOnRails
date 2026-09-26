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
    /// thread and file descriptor, and at most this many requests queue for
    /// the workers.
    pub max_connections: usize,
    /// How long a kept-alive connection may wait for its next request
    /// (`IDLE_TIMEOUT`, 20 s): Puma's `persistent_timeout`.
    pub idle_timeout: Duration,
    /// From a request's first byte to the end of its headers, however they
    /// trickle in (`HEADER_TIMEOUT`, 20 s). Past it, a 408.
    pub header_timeout: Duration,
    /// For the whole body (`BODY_TIMEOUT`, 60 s). Past it, a 408.
    pub body_timeout: Duration,
    /// For writing the whole response to a client that reads it slowly or
    /// not at all (`WRITE_TIMEOUT`, 60 s).
    pub write_timeout: Duration,
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
            max_body_bytes: 10 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// The defaults, with any of `MAX_CONNECTIONS`, `IDLE_TIMEOUT`,
    /// `HEADER_TIMEOUT`, `BODY_TIMEOUT`, `WRITE_TIMEOUT` (seconds, fractions
    /// allowed) and `MAX_BODY_BYTES` set in the environment.
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

fn seconds(var: &impl Fn(&str) -> Option<String>, name: &str, default: Duration) -> Result<Duration, BoxError> {
    let Some(text) = var(name) else { return Ok(default) };
    match text.trim().parse::<f64>().ok().filter(|s| *s > 0.0).map(Duration::try_from_secs_f64) {
        Some(Ok(duration)) => Ok(duration),
        _ => Err(format!("{name} must be a number of seconds above 0, not {text:?}").into()),
    }
}

/// A socket whose reads, or writes, share one deadline on top of the
/// timeout each call gets, so a client sending or reading a byte at a time
/// can't hold the connection past it. Without a deadline each call just
/// waits up to `step`.
pub(crate) struct Timed {
    stream: TcpStream,
    step: Duration,
    deadline: Option<Instant>,
}

impl Timed {
    pub(crate) fn new(stream: TcpStream, step: Duration) -> Self {
        Self { stream, step, deadline: None }
    }

    /// Everything from now on must be done within `limit`; None lifts it.
    pub(crate) fn within(&mut self, limit: Option<Duration>) {
        self.deadline = limit.map(|limit| Instant::now() + limit);
    }

    pub(crate) fn get_ref(&self) -> &TcpStream {
        &self.stream
    }

    /// The next call's timeout, or TimedOut once the deadline has passed.
    fn timeout(&self) -> io::Result<Duration> {
        let Some(deadline) = self.deadline else { return Ok(self.step) };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { Err(io::ErrorKind::TimedOut.into()) } else { Ok(left.min(self.step)) }
    }

    /// A call cut short once the deadline has passed is the deadline's.
    fn expired(&self, error: io::Error) -> io::Error {
        let passed = self.deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let cut = matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut);
        if passed && cut { io::ErrorKind::TimedOut.into() } else { error }
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.timeout()?))?;
        self.stream.read(buf).map_err(|error| self.expired(error))
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.timeout()?))?;
        self.stream.write(buf).map_err(|error| self.expired(error))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
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
        for (name, value) in [("MAX_CONNECTIONS", "0"), ("MAX_CONNECTIONS", "lots"), ("IDLE_TIMEOUT", "-1"), ("BODY_TIMEOUT", "NaN"), ("WRITE_TIMEOUT", "1e30")] {
            let error = Limits::from_vars(vars(&[(name, value)])).unwrap_err();
            assert!(error.to_string().starts_with(name), "{error}");
        }
    }
}

//! Just enough of Redis's protocol (RESP) for Sidekiq's queues: commands
//! out as arrays of bulk strings, replies read back. Timeouts are
//! redis-client's (Sidekiq's client): a second to connect, read or write,
//! plus however long a blocking command blocks.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq)]
pub enum Reply {
    Nil,
    Int(i64),
    Bulk(Vec<u8>),
    Array(Vec<Reply>),
    Status(String),
}

pub struct Redis {
    stream: BufReader<TcpStream>,
    /// The connection failed mid-command: what's on it can't be trusted.
    broken: bool,
}

impl Redis {
    /// `redis://[[user]:password@]host[:port][/db]`, Sidekiq's REDIS_URL.
    pub fn connect(url: &str) -> Result<Self> {
        let rest = url.strip_prefix("redis://").ok_or_else(|| failed(format!("not a redis:// URL: {url}")))?;
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let (auth, rest) = match rest.rsplit_once('@') {
            Some((auth, rest)) => (Some(auth), rest),
            None => (None, rest),
        };
        let (address, db) = rest.split_once('/').unwrap_or((rest, ""));
        let address = if address.contains(':') { address.to_string() } else { format!("{address}:6379") };
        let stream = open(&address)?;
        let mut redis = Self { stream: BufReader::new(stream), broken: false };
        let (user, password) = auth.map_or(("", ""), |auth| auth.split_once(':').unwrap_or(("", auth)));
        let (user, password) = (decode(user), decode(password));
        match (user.as_str(), password.as_str()) {
            (_, "") => {}
            ("", password) => {
                redis.command(&["AUTH", password])?;
            }
            (user, password) => {
                redis.command(&["AUTH", user, password])?;
            }
        }
        if !db.is_empty() && db != "0" {
            redis.command(&["SELECT", db])?;
        }
        Ok(redis)
    }

    /// Whether the connection failed, rather than Redis answering an error.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn command(&mut self, args: &[&str]) -> Result<Reply> {
        let args: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
        self.command_bytes(&args)
    }

    /// A command whose arguments may not be UTF-8, such as a member read
    /// back from a sorted set.
    pub fn command_bytes(&mut self, args: &[&[u8]]) -> Result<Reply> {
        self.send(args, TIMEOUT)
    }

    /// `BRPOP keys... timeout`: waits up to `timeout` seconds for a job.
    pub fn brpop(&mut self, keys: &[String], timeout: u64) -> Result<Reply> {
        let seconds = timeout.to_string();
        let args: Vec<&[u8]> = std::iter::once(b"BRPOP".as_slice())
            .chain(keys.iter().map(|key| key.as_bytes()))
            .chain(std::iter::once(seconds.as_bytes()))
            .collect();
        self.send(&args, TIMEOUT + Duration::from_secs(timeout))
    }

    fn send(&mut self, args: &[&[u8]], read_timeout: Duration) -> Result<Reply> {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
            out.extend_from_slice(arg);
            out.extend_from_slice(b"\r\n");
        }
        let stream = self.stream.get_mut();
        let written = stream.set_read_timeout(Some(read_timeout)).and_then(|()| stream.write_all(&out)).and_then(|()| stream.flush());
        if let Err(e) = written {
            return Err(self.lost(format!("writing to Redis: {e}")));
        }
        self.reply()
    }

    fn reply(&mut self) -> Result<Reply> {
        let mut line = String::new();
        match self.stream.read_line(&mut line) {
            Ok(0) => return Err(self.lost("Redis closed the connection".into())),
            Ok(_) => {}
            Err(e) => return Err(self.lost(format!("reading from Redis: {e}"))),
        }
        let line = line.trim_end_matches("\r\n").to_string();
        let (kind, rest) = line.split_at(line.len().min(1));
        let number = rest.parse::<i64>().ok();
        let unreadable = |redis: &mut Self| redis.lost(format!("a Redis reply I can't read: {line}"));
        match (kind, number) {
            ("+", _) => Ok(Reply::Status(rest.to_string())),
            // A primary that became a replica, or one going down, answers
            // with an error: the next command needs a new connection, which
            // finds the new primary, as Sidekiq's client reconnects.
            ("-", _) if ["READONLY", "MASTERDOWN", "UNBLOCKED"].iter().any(|code| rest.starts_with(code)) => {
                Err(self.lost(format!("Redis: {rest}")))
            }
            ("-", _) => Err(failed(format!("Redis: {rest}"))),
            (":", Some(n)) => Ok(Reply::Int(n)),
            ("$" | "*", Some(-1)) => Ok(Reply::Nil),
            ("$", Some(length)) if length >= 0 => {
                let mut bytes = vec![0; length as usize + 2];
                if let Err(e) = self.stream.read_exact(&mut bytes) {
                    return Err(self.lost(format!("reading from Redis: {e}")));
                }
                bytes.truncate(length as usize);
                Ok(Reply::Bulk(bytes))
            }
            ("*", Some(count)) if count >= 0 => (0..count).map(|_| self.reply()).collect::<Result<Vec<_>>>().map(Reply::Array),
            _ => Err(unreadable(self)),
        }
    }

    fn lost(&mut self, message: String) -> Error {
        self.broken = true;
        failed(message)
    }
}

/// A connection to `address`, given up on after the connect timeout.
fn open(address: &str) -> Result<TcpStream> {
    let refused = |e: std::io::Error| failed(format!("connecting to Redis at {address}: {e}"));
    let mut last = None;
    for addr in address.to_socket_addrs().map_err(refused)? {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(stream) => {
                stream.set_write_timeout(Some(TIMEOUT)).map_err(refused)?;
                return Ok(stream);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(refused(last.unwrap_or_else(|| std::io::Error::other("no address"))))
}

/// A URL's `%XX` escapes, as in a password.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn failed(message: String) -> Error {
    Error::Redis { message }
}

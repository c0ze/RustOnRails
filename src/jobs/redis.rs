//! Just enough of Redis's protocol (RESP) for Sidekiq's queues: commands
//! out as arrays of bulk strings, replies read back.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use crate::{Error, Result};

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
        let stream = TcpStream::connect(&address).map_err(|e| failed(format!("connecting to Redis at {address}: {e}")))?;
        let mut redis = Self { stream: BufReader::new(stream) };
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

    pub fn command(&mut self, args: &[&str]) -> Result<Reply> {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
            out.extend_from_slice(arg.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        let stream = self.stream.get_mut();
        stream.write_all(&out).and_then(|()| stream.flush()).map_err(|e| failed(format!("writing to Redis: {e}")))?;
        self.reply()
    }

    fn reply(&mut self) -> Result<Reply> {
        let mut line = String::new();
        if self.stream.read_line(&mut line).map_err(|e| failed(format!("reading from Redis: {e}")))? == 0 {
            return Err(failed("Redis closed the connection".into()));
        }
        let line = line.trim_end_matches("\r\n");
        let (kind, rest) = line.split_at(line.len().min(1));
        let number = || rest.parse::<i64>().map_err(|_| failed(format!("a Redis reply I can't read: {line}")));
        match kind {
            "+" => Ok(Reply::Status(rest.to_string())),
            "-" => Err(failed(format!("Redis: {rest}"))),
            ":" => Ok(Reply::Int(number()?)),
            "$" => match number()? {
                -1 => Ok(Reply::Nil),
                length if length < 0 => Err(failed(format!("a Redis reply I can't read: {line}"))),
                length => {
                    let mut bytes = vec![0; length as usize + 2];
                    self.stream.read_exact(&mut bytes).map_err(|e| failed(format!("reading from Redis: {e}")))?;
                    bytes.truncate(length as usize);
                    Ok(Reply::Bulk(bytes))
                }
            },
            "*" => match number()? {
                -1 => Ok(Reply::Nil),
                count => (0..count).map(|_| self.reply()).collect::<Result<Vec<_>>>().map(Reply::Array),
            },
            _ => Err(failed(format!("a Redis reply I can't read: {line}"))),
        }
    }
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

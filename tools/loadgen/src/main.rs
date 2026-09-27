//! A small HTTP/1.1 load generator for the Rails-vs-Rust benchmark:
//! `loadgen URL CONCURRENCY SECONDS [HEADER...]` opens CONCURRENCY
//! keep-alive connections, sends GETs for SECONDS, and prints requests per
//! second and p50/p99 latency of the 200 responses that finished within
//! SECONDS. Each HEADER (`"X-Api-Token: abc"`) goes on every request.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

type Connection = BufReader<TcpStream>;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args[4..].iter().any(|h| !h.contains(':') || h.contains(['\r', '\n'])) {
        eprintln!("usage: loadgen URL CONCURRENCY SECONDS [\"Name: value\"...]");
        std::process::exit(2);
    }
    let (host, path) = split_url(&args[1]);
    let concurrency: usize = args[2].parse().expect("CONCURRENCY is a number");
    let seconds: u64 = args[3].parse().expect("SECONDS is a number");
    let request = request(&host, &path, &args[4..]);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let workers: Vec<_> = (0..concurrency)
        .map(|_| {
            let (host, request) = (host.clone(), request.clone());
            std::thread::spawn(move || run(&host, &request, deadline))
        })
        .collect();
    let mut latencies = Vec::new();
    let mut errors = 0;
    for worker in workers {
        let (done, failed) = worker.join().expect("worker thread");
        latencies.extend(done);
        errors += failed;
    }
    latencies.sort();
    let percentile = |p: f64| {
        let index = ((latencies.len() as f64 * p) as usize).min(latencies.len().saturating_sub(1));
        latencies.get(index).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    };
    println!(
        "{:9.0} req/s  p50 {:6.2} ms  p99 {:6.2} ms  errors {errors}",
        latencies.len() as f64 / seconds as f64,
        percentile(0.50),
        percentile(0.99)
    );
}

/// `http://127.0.0.1:3000/posts` → ("127.0.0.1:3000", "/posts")
fn split_url(url: &str) -> (String, String) {
    let rest = url.strip_prefix("http://").expect("an http:// URL");
    match rest.find('/') {
        Some(slash) => (rest[..slash].to_string(), rest[slash..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// The GET every connection sends, with the extra headers. It asks for
/// JSON unless they name an Accept of their own (an HTML page).
fn request(host: &str, path: &str, headers: &[String]) -> String {
    let extra: String = headers.iter().map(|h| format!("{h}\r\n")).collect();
    let own = headers.iter().any(|h| h.split(':').next().is_some_and(|name| name.trim().eq_ignore_ascii_case("accept")));
    let accept = if own { "" } else { "Accept: application/json\r\n" };
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{accept}{extra}\r\n")
}

fn run(host: &str, request: &str, deadline: Instant) -> (Vec<Duration>, usize) {
    let (mut latencies, mut errors, mut connection) = (Vec::new(), 0, None::<Connection>);
    let Some(address) = host.to_socket_addrs().ok().and_then(|mut addresses| addresses.next()) else {
        return (latencies, 1);
    };
    loop {
        let started = Instant::now();
        // Every connect, write and read is bounded by what's left of the run.
        let Some(left) = deadline.checked_duration_since(started).filter(|left| !left.is_zero()) else { break };
        let stream = match connection.take() {
            Some(stream) => stream,
            None => match TcpStream::connect_timeout(&address, left) {
                Ok(stream) => BufReader::new(stream),
                Err(_) => {
                    errors += 1;
                    continue;
                }
            },
        };
        let result = exchange(stream, request, left);
        // A request still in flight at the deadline isn't part of the run.
        if Instant::now() >= deadline {
            break;
        }
        match result {
            Ok((stream, status, keep_alive)) => {
                if status == 200 { latencies.push(started.elapsed()) } else { errors += 1 }
                if keep_alive {
                    connection = Some(stream);
                }
            }
            Err(_) => errors += 1,
        }
    }
    (latencies, errors)
}

fn exchange(mut reader: Connection, request: &str, left: Duration) -> io::Result<(Connection, u16, bool)> {
    let stream = reader.get_mut();
    stream.set_read_timeout(Some(left))?;
    stream.set_write_timeout(Some(left))?;
    stream.write_all(request.as_bytes())?;
    let (status, keep_alive) = read_response(&mut reader)?;
    Ok((reader, status, keep_alive))
}

/// Reads one whole response, Content-Length or chunked, and returns its
/// status and whether the connection stays open.
fn read_response(reader: &mut impl BufRead) -> io::Result<(u16, bool)> {
    let status_line = read_line(reader)?;
    let status = status_line.get(9..12).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::other("bad status line"))?;
    let (mut length, mut chunked, mut keep_alive) = (0, false, true);
    loop {
        let header = read_line(reader)?.to_ascii_lowercase();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or_else(|| io::Error::other("bad header line"))?;
        let mut tokens = value.split(',').map(str::trim);
        match name {
            "content-length" => length = value.trim().parse().map_err(io::Error::other)?,
            "transfer-encoding" => chunked = tokens.any(|t| t == "chunked"),
            "connection" => keep_alive &= !tokens.any(|t| t == "close"),
            _ => {}
        }
    }
    if chunked { read_chunks(reader)? } else { skip(reader, length)? }
    Ok((status, keep_alive))
}

/// One line without its line ending. Input that ends first is an error.
fn read_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 || !line.ends_with('\n') {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Chunks (with any extensions), then trailers up to the closing blank line.
fn read_chunks(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let line = read_line(reader)?;
        let size = u64::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(io::Error::other)?;
        if size == 0 {
            break;
        }
        skip(reader, size)?;
        if !read_line(reader)?.is_empty() {
            return Err(io::Error::other("chunk longer than its size"));
        }
    }
    while !read_line(reader)?.is_empty() {}
    Ok(())
}

fn skip(reader: &mut impl BufRead, bytes: u64) -> io::Result<()> {
    if io::copy(&mut reader.by_ref().take(bytes), &mut io::sink())? != bytes {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::sync::mpsc::channel;

    #[test]
    fn test_extra_headers_go_before_the_blank_line() {
        let api = request("h:1", "/projects", &["X-Api-Token: abc".to_string()]);
        assert_eq!("GET /projects HTTP/1.1\r\nHost: h:1\r\nAccept: application/json\r\nX-Api-Token: abc\r\n\r\n", api);
        let page = request("h:1", "/shop", &["accept: text/html".to_string()]);
        assert_eq!("GET /shop HTTP/1.1\r\nHost: h:1\r\naccept: text/html\r\n\r\n", page);
    }

    #[test]
    fn test_truncated_headers_are_an_error() {
        let mut reader = Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n".to_vec());
        assert!(read_response(&mut reader).is_err());
    }

    #[test]
    fn test_chunks_with_extensions_and_trailers_leave_the_next_response_intact() {
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;x=1\r\nhi\r\n0\r\nT: 1\r\n\r\nHTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        let mut reader = Cursor::new(raw.as_bytes().to_vec());
        assert_eq!(200, read_response(&mut reader).unwrap().0);
        assert_eq!((204, false), read_response(&mut reader).unwrap());
    }

    #[test]
    fn test_a_silent_server_does_not_hang_the_run() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        // Accepts and never answers.
        std::thread::spawn(move || {
            let _held: Vec<_> = listener.incoming().collect();
        });
        let (done, finished) = channel();
        std::thread::spawn(move || {
            done.send(run(&host, "GET / HTTP/1.1\r\n\r\n", Instant::now() + Duration::from_millis(300))).ok();
        });
        let (latencies, _) = finished.recv_timeout(Duration::from_secs(3)).expect("run returned");
        assert!(latencies.is_empty());
    }
}

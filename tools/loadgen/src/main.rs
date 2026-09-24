//! A small HTTP/1.1 load generator for the Rails-vs-Rust benchmark:
//! `loadgen URL CONCURRENCY SECONDS` opens CONCURRENCY keep-alive
//! connections, sends GETs for SECONDS, and prints requests per second and
//! p50/p99 latency of the 200 responses.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

type Connection = BufReader<TcpStream>;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: loadgen URL CONCURRENCY SECONDS");
        std::process::exit(2);
    }
    let (host, path) = split_url(&args[1]);
    let concurrency: usize = args[2].parse().expect("CONCURRENCY is a number");
    let seconds: u64 = args[3].parse().expect("SECONDS is a number");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let workers: Vec<_> = (0..concurrency)
        .map(|_| {
            let (host, path) = (host.clone(), path.clone());
            std::thread::spawn(move || run(&host, &path, deadline))
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

fn run(host: &str, path: &str, deadline: Instant) -> (Vec<Duration>, usize) {
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\r\n");
    let (mut latencies, mut errors, mut connection) = (Vec::new(), 0, None::<Connection>);
    while Instant::now() < deadline {
        let started = Instant::now();
        let stream = match connection.take() {
            Some(stream) => stream,
            None => match TcpStream::connect(host) {
                Ok(stream) => BufReader::new(stream),
                Err(_) => {
                    errors += 1;
                    continue;
                }
            },
        };
        match exchange(stream, &request) {
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

/// Sends one request and reads the whole response (Content-Length or chunked).
fn exchange(mut reader: Connection, request: &str) -> io::Result<(Connection, u16, bool)> {
    reader.get_mut().write_all(request.as_bytes())?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line.get(9..12).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::other("bad status line"))?;
    let (mut length, mut chunked, mut keep_alive) = (0, false, true);
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let header = line.trim_end().to_ascii_lowercase();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        chunked |= header == "transfer-encoding: chunked";
        keep_alive &= header != "connection: close";
    }
    if chunked {
        read_chunks(&mut reader)?;
    } else {
        reader.read_exact(&mut vec![0; length])?;
    }
    Ok((reader, status, keep_alive))
}

fn read_chunks(reader: &mut Connection) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let size = usize::from_str_radix(line.trim(), 16).map_err(io::Error::other)?;
        reader.read_exact(&mut vec![0; size + 2])?;
        if size == 0 {
            return Ok(());
        }
    }
}

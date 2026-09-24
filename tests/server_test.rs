mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use rustonrails::server::{self, Config};
use rustonrails::{Json, Request, Response, Router, json};

fn send(address: SocketAddr, request: &str) -> (u16, Json) {
    let mut stream = TcpStream::connect(address).unwrap();
    // A blocked server fails the test instead of hanging it.
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status = raw[9..12].parse().unwrap();
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Json::Null))
}

fn get(address: SocketAddr, path: &str) -> (u16, Json) {
    send(address, &format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"))
}

fn start(workers: usize) -> server::Running {
    support::prepare();
    let router = Router::new()
        .get("/up", Box::new(|_: &mut Request| Response::json(200, json!({"ok": true}))))
        .get("/boom", Box::new(|_: &mut Request| panic!("boom")))
        .get("/users/count", Box::new(|req: &mut Request| {
            // Errors come back as a response, the way `dispatch` handles them.
            match req.ctx.query("SELECT COUNT(*) FROM users", &[]) {
                Ok(rows) => Response::json(200, json!(rows[0].get::<_, i64>(0))),
                Err(_) => rustonrails::error_page(500),
            }
        }))
        .get("/pid", Box::new(|req: &mut Request| {
            let pid: i32 = req.ctx.query("SELECT pg_backend_pid()", &[]).unwrap()[0].get(0);
            Response::json(200, json!(pid))
        }))
        .get("/big", Box::new(|_: &mut Request| Response::json(200, json!("x".repeat(4096)))))
        .post("/echo", Box::new(|req: &mut Request| {
            Response::json(201, json!({"name": req.params.get("name"), "page": req.params.get("page")}))
        }));
    server::start(router, Config { address: "127.0.0.1:0".into(), database_url: support::url(), workers }).unwrap()
}

#[test]
fn test_server_answers_routes_and_404s() {
    let running = start(2);
    assert_eq!((200, json!({"ok": true})), get(running.address, "/up"));
    assert_eq!((404, json!({"status": 404, "error": "Not Found"})), get(running.address, "/nowhere"));
    assert_eq!(200, get(running.address, "/users/count").0);
    running.stop();
}

#[test]
fn test_server_parses_json_bodies_and_query_strings() {
    let running = start(2);
    let body = r#"{"name":"Ann"}"#;
    let request = format!(
        "POST /echo?page=2 HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    assert_eq!((201, json!({"name": "Ann", "page": "2"})), send(running.address, &request));
    running.stop();
}

#[test]
fn test_server_survives_a_panicking_handler() {
    // One worker, so the requests after the panic land on the worker that panicked.
    let running = start(1);
    assert_eq!((500, json!({"status": 500, "error": "Internal Server Error"})), get(running.address, "/boom"));
    for _ in 0..3 {
        assert_eq!(200, get(running.address, "/users/count").0);
    }
    running.stop();
}

#[test]
fn test_a_dead_connection_is_replaced() {
    let running = start(1);
    let (_, pid) = get(running.address, "/pid");
    let mut admin = postgres::Client::connect(&support::url(), postgres::NoTls).unwrap();
    admin.execute("SELECT pg_terminate_backend($1)", &[&(pid.as_i64().unwrap() as i32)]).unwrap();
    // The driver only notices the dead socket when it next uses it, so that
    // request fails; the worker must reconnect instead of failing forever.
    get(running.address, "/users/count");
    assert_eq!(200, get(running.address, "/users/count").0);
    assert_eq!(200, get(running.address, "/users/count").0);
    running.stop();
}

#[test]
fn test_a_stalled_upload_does_not_block_other_requests() {
    let running = start(1);
    let mut stalled = TcpStream::connect(running.address).unwrap();
    let head = "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 5000\r\n\r\n{";
    stalled.write_all(head.as_bytes()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(200, get(running.address, "/up").0);
    drop(stalled);
    running.stop();
}

#[test]
fn test_an_oversized_body_is_413() {
    let running = start(1);
    let request = "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 20971520\r\nConnection: close\r\n\r\n";
    assert_eq!(413, send(running.address, request).0);
    running.stop();
}

/// Reads one response off a keep-alive connection and returns its body length.
fn read_response(stream: &mut TcpStream) -> usize {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).unwrap();
    length
}

#[test]
fn test_multi_segment_responses_are_not_held_back_by_nagle() {
    // Headers and body leave in separate writes; without TCP_NODELAY the
    // body waits for the client's delayed ACK, about 40 ms per request.
    let running = start(1);
    let mut stream = TcpStream::connect(running.address).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..10 {
        stream.write_all(b"GET /big HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
        assert!(read_response(&mut stream) > 4096);
    }
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_millis(150), "10 requests took {elapsed:?}");
    running.stop();
}

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
        .get("/token", Box::new(|req: &mut Request| Response::json(200, json!(req.header("x-api-token")))))
        .post("/echo", Box::new(|req: &mut Request| {
            Response::json(201, json!({"name": req.params.get("name"), "page": req.params.get("page")}))
        }));
    server::start(router, Config { address: "127.0.0.1:0".into(), database_url: support::url(), workers, secret_key_base: None, redis_url: None }).unwrap()
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

/// Writes raw bytes, optionally half-closes, and returns everything the
/// server sends back before it closes or goes quiet.
fn exchange_raw(address: SocketAddr, bytes: &[u8], half_close: bool) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    stream.write_all(bytes).unwrap();
    if half_close {
        stream.shutdown(std::net::Shutdown::Write).unwrap();
    }
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    String::from_utf8_lossy(&raw).into_owned()
}

#[test]
fn test_a_huge_content_length_is_refused_and_the_server_survives() {
    let running = start(1);
    let head = "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 1000000000000\r\n\r\n{";
    let raw = exchange_raw(running.address, head.as_bytes(), true);
    assert!(raw.starts_with("HTTP/1.1 413 "), "{raw}");
    assert_eq!(200, get(running.address, "/up").0);
    running.stop();
}

#[test]
fn test_a_truncated_body_is_not_dispatched() {
    let running = start(1);
    let body = r#"{"name":"Ann"}"#;
    let request = format!(
        "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n{body}"
    );
    let raw = exchange_raw(running.address, request.as_bytes(), true);
    assert!(!raw.contains(" 201 "), "{raw}");
    running.stop();
}

#[test]
fn test_chunked_bodies_are_decoded() {
    let running = start(1);
    let request = "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5;ext=1\r\n{\"nam\r\n9\r\ne\":\"Ann\"}\r\n0\r\nX-Trailer: yes\r\n\r\n";
    assert_eq!((201, json!({"name": "Ann", "page": null})), send(running.address, request));
    running.stop();
}

#[test]
fn test_content_length_with_transfer_encoding_is_400() {
    let running = start(1);
    let request = "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    let raw = exchange_raw(running.address, request.as_bytes(), false);
    assert!(raw.starts_with("HTTP/1.1 400 "), "{raw}");
    running.stop();
}

#[test]
fn test_expect_continue_gets_100_before_the_body() {
    let running = start(1);
    let body = r#"{"name":"Ann"}"#;
    let mut stream = TcpStream::connect(running.address).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    let head = format!(
        "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut interim = [0u8; 25];
    stream.read_exact(&mut interim).unwrap();
    assert_eq!(b"HTTP/1.1 100 Continue\r\n\r\n", &interim);
    stream.write_all(body.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 201 "), "{raw}");
    running.stop();
}

#[test]
fn test_pipelined_requests_get_answers_in_order() {
    let running = start(1);
    let mut stream = TcpStream::connect(running.address).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    stream.write_all(b"GET /big HTTP/1.1\r\nHost: test\r\n\r\nGET /big HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
    assert!(read_response(&mut stream) > 4096);
    assert!(read_response(&mut stream) > 4096);
    running.stop();
}

#[test]
fn test_head_runs_the_get_route_without_a_body() {
    let running = start(1);
    let raw = exchange_raw(running.address, b"HEAD /up HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n", false);
    assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
    assert!(raw.contains("Content-Length: 11\r\n"), "{raw}");
    assert!(raw.ends_with("\r\n\r\n"), "{raw}");
    running.stop();
}

fn post_echo(address: SocketAddr, content_type: &str, body: &str) -> (u16, Json) {
    let request = format!(
        "POST /echo HTTP/1.1\r\nHost: test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    send(address, &request)
}

#[test]
fn test_malformed_json_is_400_and_the_action_never_runs() {
    let running = start(1);
    assert_eq!((400, json!({"status": 400, "error": "Bad Request"})), post_echo(running.address, "application/json", "{"));
    // Rails doesn't parse an empty body, so it isn't malformed.
    assert_eq!((201, json!({"name": null, "page": null})), post_echo(running.address, "application/json", ""));
    // Routing comes first, as in Rails: no route is still a 404.
    let request = "POST /nowhere HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{";
    assert_eq!(404, send(running.address, request).0);
    running.stop();
}

#[test]
fn test_only_json_media_types_are_parsed_as_json() {
    let running = start(1);
    let body = r#"{"name":"Ann"}"#;
    assert_eq!((201, json!({"name": null, "page": null})), post_echo(running.address, "application/jsonp", body));
    assert_eq!((201, json!({"name": "Ann", "page": null})), post_echo(running.address, "Application/JSON; charset=utf-8", body));
    assert_eq!((201, json!({"name": "Ann", "page": null})), post_echo(running.address, "text/x-json", body));
    running.stop();
}

#[test]
fn test_form_bodies_become_params() {
    let running = start(1);
    let (status, echoed) = post_echo(running.address, "application/x-www-form-urlencoded", "name=Ann+Lee&page=3");
    assert_eq!((201, json!({"name": "Ann Lee", "page": "3"})), (status, echoed));
    running.stop();
}

#[test]
fn test_headers_reach_the_request_case_insensitively() {
    let running = start(1);
    let request = "GET /token HTTP/1.1\r\nHost: test\r\nX-Api-Token: abc\r\nConnection: close\r\n\r\n";
    assert_eq!((200, json!("abc")), send(running.address, request));
    assert_eq!((200, json!(null)), get(running.address, "/token"));
    running.stop();
}

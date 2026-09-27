mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use rustonrails::server::{self, Config};
use rustonrails::{Json, Limits, Request, Response, Router, json};

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
    start_with(workers, Limits::default())
}

fn start_with(workers: usize, limits: Limits) -> server::Running {
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
    server::start(
        router,
        Config { address: "127.0.0.1:0".into(), database_url: support::url(), workers, limits, secret_key_base: None, redis_url: None },
    )
    .unwrap()
}

#[test]
fn test_server_answers_routes_and_404s() {
    let running = start(2);
    assert_eq!((200, json!({"ok": true})), get(running.address, "/up"));
    assert_eq!((404, json!({"status": 404, "error": "Not Found"})), get(running.address, "/nowhere.json"));
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
    let boom = "GET /boom HTTP/1.1\r\nHost: test\r\nAccept: application/json\r\nConnection: close\r\n\r\n";
    assert_eq!((500, json!({"status": 500, "error": "Internal Server Error"})), send(running.address, boom));
    for _ in 0..3 {
        assert_eq!(200, get(running.address, "/users/count").0);
    }
    running.stop();
}

/// A panic outside a transaction leaves the connection clean, so the
/// worker keeps it rather than reconnecting for the next request.
#[test]
fn test_a_panic_outside_a_transaction_keeps_the_connection() {
    let running = start(1);
    let (_, before) = get(running.address, "/pid");
    assert_eq!(500, get(running.address, "/boom").0);
    assert_eq!(before, get(running.address, "/pid").1);
    running.stop();
}

/// Methods are case-sensitive, so `get` isn't routed as GET.
#[test]
fn test_a_lowercase_method_is_not_get() {
    let running = start(1);
    let request = "get /up HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n";
    assert_eq!(404, send(running.address, request).0);
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

/// Sends `head` and then `trickle` a byte at a time, 50 ms apart, as a
/// slowloris client would; returns the status that comes back and how long
/// it took.
fn trickled(address: SocketAddr, head: &str, trickle: &str) -> (u16, Duration) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let started = Instant::now();
    stream.write_all(head.as_bytes()).unwrap();
    let mut raw = Vec::new();
    for byte in trickle.bytes().cycle().take(200) {
        if stream.write_all(&[byte]).is_err() {
            break;
        }
        match stream.read_to_end(&mut raw) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
    }
    let status = String::from_utf8_lossy(&raw).get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, started.elapsed())
}

/// Headers or a body trickling in a byte at a time each run against a
/// deadline, not the idle timeout each read would reset: past it, a 408.
#[test]
fn test_a_request_that_trickles_in_is_cut_off_with_a_408() {
    let limits = Limits { header_timeout: Duration::from_millis(300), body_timeout: Duration::from_millis(300), ..Limits::default() };
    let running = start_with(1, limits);
    let (status, took) = trickled(running.address, "GET /up HTTP/1.1\r\nX-Slow: ", "a");
    assert_eq!(408, status);
    assert!(took < Duration::from_secs(3), "{took:?}");
    let head = "POST /echo HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 5000\r\n\r\n";
    let (status, took) = trickled(running.address, head, " ");
    assert_eq!(408, status);
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert_eq!(200, get(running.address, "/up").0);
    running.stop();
}

/// A body that keeps moving at MIN_RATE or faster gets the time it needs:
/// here 2,000 bytes over a second against a 200 ms BODY_TIMEOUT, at twice
/// the minimum rate.
#[test]
fn test_a_slow_but_steady_upload_is_served() {
    let limits = Limits { body_timeout: Duration::from_millis(200), min_rate: 1000, ..Limits::default() };
    let running = start_with(1, limits);
    let body = format!("{{\"name\":\"{}\"}}", "a".repeat(1989));
    assert_eq!(2000, body.len());
    let mut stream = TcpStream::connect(running.address).unwrap();
    let head = "POST /echo HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 2000\r\nConnection: close\r\n\r\n";
    stream.write_all(head.as_bytes()).unwrap();
    for chunk in body.as_bytes().chunks(100) {
        stream.write_all(chunk).unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 201"), "{raw}");
    running.stop();
}

/// IDLE_TIMEOUT is the wait between requests; a pause inside one is the
/// header or body deadline's to judge.
#[test]
fn test_a_pause_inside_a_request_is_not_idleness() {
    let limits = Limits { idle_timeout: Duration::from_millis(100), ..Limits::default() };
    let running = start_with(1, limits);
    let mut stream = TcpStream::connect(running.address).unwrap();
    stream.write_all(b"POST /echo HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 12\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    stream.write_all(b"Connection: close\r\n\r\n{\"name\":").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    stream.write_all(b"\"A\"}").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 201"), "{raw}");
    running.stop();
}

/// Past max_connections a new connection is a 503, and a slot frees when
/// its connection ends.
#[test]
fn test_connections_past_the_limit_are_turned_away() {
    let running = start_with(1, Limits { max_connections: 2, ..Limits::default() });
    let held: Vec<TcpStream> = (0..2).map(|_| TcpStream::connect(running.address).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(503, get(running.address, "/up").0);
    drop(held);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(200, get(running.address, "/up").0);
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
        "POST /echo HTTP/1.1\r\nHost: test\r\nAccept: application/json\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

/// A panic, or no database, fails outside the app, but Rails' exceptions
/// app and SSL middleware still answer: the app's 500 page to a browser,
/// JSON to a JSON request, HSTS on both.
#[test]
fn test_failures_outside_the_app_go_through_its_middleware() {
    support::prepare();
    let router = Router::new()
        .get("/boom", Box::new(|_: &mut Request| -> Response { panic!("boom") }))
        .force_ssl()
        .public_page(500, "<h1>Failed</h1>");
    let raw = |running: &server::Running, request: &str| {
        let mut stream = TcpStream::connect(running.address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        raw
    };
    let config = |database_url: String| Config {
        address: "127.0.0.1:0".into(),
        database_url,
        workers: 1,
        limits: Limits::default(),
        secret_key_base: None,
        redis_url: None,
    };
    let running = server::start(router, config(support::url())).unwrap();
    let page = raw(&running, "GET /boom HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    assert!(page.starts_with("HTTP/1.1 500"), "{page}");
    assert!(page.contains("Strict-Transport-Security: max-age=63072000; includeSubDomains"), "{page}");
    assert!(page.ends_with("<h1>Failed</h1>"), "{page}");
    let json = raw(&running, "GET /boom HTTP/1.1\r\nHost: test\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    assert!(json.ends_with(r#"{"status":500,"error":"Internal Server Error"}"#), "{json}");
    running.stop();

    // No database: the same page, without ever reaching a handler.
    let router = Router::new().get("/boom", Box::new(|_: &mut Request| Response::head(200))).force_ssl().public_page(500, "<h1>Failed</h1>");
    let running = server::start(router, config("postgres://nobody@127.0.0.1:1/none".into())).unwrap();
    let page = raw(&running, "GET /boom HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    assert!(page.starts_with("HTTP/1.1 500") && page.ends_with("<h1>Failed</h1>"), "{page}");
    assert!(page.contains("Strict-Transport-Security"), "{page}");
    // A format the body sends counts, as Rails merges body params in.
    let form = "POST /boom HTTP/1.1\r\nHost: test\r\nContent-Type: application/x-www-form-urlencoded\r\n\
                Content-Length: 11\r\nConnection: close\r\n\r\nformat=json";
    assert!(raw(&running, form).ends_with(r#"{"status":500,"error":"Internal Server Error"}"#));
    // The query's format, even one that isn't a string, replaces the body's.
    let both = "POST /boom?format%5B%5D=html HTTP/1.1\r\nHost: test\r\nAccept: text/html\r\n\
                Content-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"format\":\"json\"}";
    assert!(raw(&running, both).ends_with("<h1>Failed</h1>"));
    running.stop();
}

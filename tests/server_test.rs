mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use rustonrails::server::{self, Config};
use rustonrails::{Json, Request, Response, Router, json};

fn send(address: SocketAddr, request: &str) -> (u16, Json) {
    let mut stream = TcpStream::connect(address).unwrap();
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

fn start() -> server::Running {
    support::prepare();
    let router = Router::new()
        .get("/up", Box::new(|_: &mut Request| Response::json(200, json!({"ok": true}))))
        .get("/boom", Box::new(|_: &mut Request| panic!("boom")))
        .get("/users/count", Box::new(|req: &mut Request| {
            let count: i64 = req.ctx.query("SELECT COUNT(*) FROM users", &[]).unwrap()[0].get(0);
            Response::json(200, json!(count))
        }))
        .post("/echo", Box::new(|req: &mut Request| {
            Response::json(201, json!({"name": req.params.get("name"), "page": req.params.get("page")}))
        }));
    server::start(router, Config { address: "127.0.0.1:0".into(), database_url: support::url(), workers: 2 }).unwrap()
}

#[test]
fn test_server_answers_routes_and_404s() {
    let running = start();
    assert_eq!((200, json!({"ok": true})), get(running.address, "/up"));
    assert_eq!((404, json!({"status": 404, "error": "Not Found"})), get(running.address, "/nowhere"));
    assert_eq!(200, get(running.address, "/users/count").0);
    running.stop();
}

#[test]
fn test_server_parses_json_bodies_and_query_strings() {
    let running = start();
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
    let running = start();
    assert_eq!((500, json!({"status": 500, "error": "Internal Server Error"})), get(running.address, "/boom"));
    for _ in 0..3 {
        assert_eq!(200, get(running.address, "/users/count").0);
    }
    running.stop();
}

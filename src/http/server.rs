use std::io::Read;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread::JoinHandle;

use postgres::{Client, NoTls};
use serde_json::Value as Json;

use super::{Request, Response, Router, error_page};
use crate::Ctx;

/// Where to listen, which database to use, and how many worker threads to
/// run: Puma's threads, each with its own connection.
pub struct Config {
    pub address: String,
    pub database_url: String,
    pub workers: usize,
}

pub struct Running {
    pub address: SocketAddr,
    server: Arc<tiny_http::Server>,
    workers: Vec<JoinHandle<()>>,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Starts listening and returns once the workers are running.
pub fn start(router: Router, config: Config) -> Result<Running, BoxError> {
    let server = Arc::new(tiny_http::Server::http(config.address.as_str())?);
    let address = server.server_addr().to_ip().ok_or("server has no IP address")?;
    let router = Arc::new(router);
    let workers = (0..config.workers.max(1))
        .map(|_| {
            let (server, router, url) = (server.clone(), router.clone(), config.database_url.clone());
            std::thread::spawn(move || work(&server, &router, &url))
        })
        .collect();
    Ok(Running { address, server, workers })
}

impl Running {
    /// Serves until the process is stopped.
    pub fn join(self) {
        for worker in self.workers {
            worker.join().ok();
        }
    }

    pub fn stop(self) {
        for _ in &self.workers {
            self.server.unblock();
        }
        self.join();
    }
}

fn work(server: &tiny_http::Server, router: &Router, url: &str) {
    let mut client: Option<Client> = None;
    while let Ok(mut incoming) = server.recv() {
        let connection = match client.take() {
            Some(client) => Ok(client),
            None => Client::connect(url, NoTls),
        };
        let response = match connection {
            Ok(connection) => {
                let (response, kept) = handle(router, connection, &mut incoming);
                client = kept;
                response
            }
            Err(_) => error_page(500),
        };
        incoming.respond(to_tiny(response)).ok();
    }
}

/// Runs one request in a fresh `Ctx`. A panic becomes a 500 and costs the
/// connection, since the panic may have left it mid-transaction.
fn handle(router: &Router, client: Client, incoming: &mut tiny_http::Request) -> (Response, Option<Client>) {
    let mut req = build_request(Ctx::new(client), incoming);
    match catch_unwind(AssertUnwindSafe(|| router.call(&mut req))) {
        Ok(response) => (response, Some(req.ctx.into_client())),
        Err(_) => (error_page(500), None),
    }
}

fn build_request(ctx: Ctx, incoming: &mut tiny_http::Request) -> Request {
    let url = incoming.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let content_type = incoming
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string());
    let mut body = String::new();
    incoming.as_reader().read_to_string(&mut body).ok();
    let mut req = Request::new(ctx, incoming.method().as_str(), path).with_query(query);
    if content_type.as_deref().is_some_and(|t| t.starts_with("application/json")) {
        req = req.with_json(serde_json::from_str(&body).unwrap_or(Json::Null));
    }
    req.content_type = content_type;
    req
}

fn to_tiny(response: Response) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut tiny = tiny_http::Response::from_data(response.body).with_status_code(response.status);
    if let Some(content_type) = response.content_type {
        let header = tiny_http::Header::from_bytes("Content-Type", content_type).expect("valid header");
        tiny = tiny.with_header(header);
    }
    tiny
}

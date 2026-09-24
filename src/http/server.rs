use std::io::Read;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use postgres::{Client, NoTls};
use serde_json::Value as Json;

use super::{Request, Response, Router, error_page};
use crate::Ctx;

/// The largest request body accepted; bigger ones get a 413.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

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
    jobs: Sender<Job>,
    intake: JoinHandle<()>,
    workers: Vec<JoinHandle<()>>,
}

/// A request whose body has been read in full, or a signal to stop.
enum Job {
    Serve(tiny_http::Request, String),
    Stop,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Starts listening and returns once the workers are running. One intake
/// thread accepts connections and reads each body on its own short-lived
/// thread, so a client that stalls mid-upload holds that thread, never a
/// database worker; workers only see requests that are fully read.
pub fn start(router: Router, config: Config) -> Result<Running, BoxError> {
    let server = Arc::new(tiny_http::Server::http(config.address.as_str())?);
    let address = server.server_addr().to_ip().ok_or("server has no IP address")?;
    let (jobs, queue) = channel::<Job>();
    let queue = Arc::new(Mutex::new(queue));
    let router = Arc::new(router);
    let workers = (0..config.workers.max(1))
        .map(|_| {
            let (queue, router, url) = (queue.clone(), router.clone(), config.database_url.clone());
            std::thread::spawn(move || work(&queue, &router, &url))
        })
        .collect();
    let intake = {
        let (server, jobs) = (server.clone(), jobs.clone());
        std::thread::spawn(move || accept(&server, &jobs))
    };
    Ok(Running { address, server, jobs, intake, workers })
}

impl Running {
    /// Serves until the process is stopped.
    pub fn join(self) {
        self.intake.join().ok();
        for worker in self.workers {
            worker.join().ok();
        }
    }

    /// Stops accepting, lets each worker finish its current request, then
    /// returns. Reader threads stuck on stalled clients are left behind.
    pub fn stop(self) {
        self.server.unblock();
        for _ in &self.workers {
            self.jobs.send(Job::Stop).ok();
        }
        self.join();
    }
}

fn accept(server: &tiny_http::Server, jobs: &Sender<Job>) {
    while let Ok(mut incoming) = server.recv() {
        let jobs = jobs.clone();
        std::thread::spawn(move || match read_body(&mut incoming) {
            Ok(body) => {
                jobs.send(Job::Serve(incoming, body)).ok();
            }
            Err(status) => {
                incoming.respond(to_tiny(error_page(status))).ok();
            }
        });
    }
}

/// The whole body, or the status to refuse it with.
fn read_body(incoming: &mut tiny_http::Request) -> Result<String, u16> {
    if incoming.body_length().is_some_and(|length| length > MAX_BODY_BYTES) {
        return Err(413);
    }
    let mut body = Vec::new();
    let limit = MAX_BODY_BYTES as u64 + 1;
    incoming.as_reader().take(limit).read_to_end(&mut body).map_err(|_| 400u16)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(413);
    }
    String::from_utf8(body).map_err(|_| 400u16)
}

fn work(queue: &Mutex<Receiver<Job>>, router: &Router, url: &str) {
    let mut client: Option<Client> = None;
    loop {
        let job = queue.lock().map(|queue| queue.recv());
        let Ok(Ok(Job::Serve(incoming, body))) = job else { return };
        // A connection the database closed (restart, failover, idle kill)
        // is replaced rather than reused.
        let connection = match client.take().filter(|c| !c.is_closed()) {
            Some(client) => Ok(client),
            None => Client::connect(url, NoTls),
        };
        let response = match connection {
            Ok(connection) => {
                let (response, kept) = handle(router, connection, &incoming, body);
                client = kept.filter(|c| !c.is_closed());
                response
            }
            Err(error) => {
                eprintln!("database connection failed: {error}");
                error_page(500)
            }
        };
        incoming.respond(to_tiny(response)).ok();
    }
}

/// Runs one request in a fresh `Ctx`. A panic becomes a 500 and costs the
/// connection, since the panic may have left it mid-transaction.
fn handle(router: &Router, client: Client, incoming: &tiny_http::Request, body: String) -> (Response, Option<Client>) {
    let mut req = build_request(Ctx::new(client), incoming, &body);
    match catch_unwind(AssertUnwindSafe(|| router.call(&mut req))) {
        Ok(response) => (response, Some(req.ctx.into_client())),
        Err(_) => (error_page(500), None),
    }
}

fn build_request(ctx: Ctx, incoming: &tiny_http::Request, body: &str) -> Request {
    let url = incoming.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let content_type = incoming
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string());
    let mut req = Request::new(ctx, incoming.method().as_str(), path).with_query(query);
    if content_type.as_deref().is_some_and(|t| t.starts_with("application/json")) {
        req = req.with_json(serde_json::from_str(body).unwrap_or(Json::Null));
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

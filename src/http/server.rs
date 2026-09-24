use std::io::{self, BufReader, Read};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use postgres::{Client, NoTls};

use super::wire::{self, WireError};
use super::{Request, Response, Router, error_page};
use crate::Ctx;

/// The largest request body accepted; bigger ones get a 413.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// How long a connection may sit idle between requests, or stall in the
/// middle of one, before it's closed: Puma's `persistent_timeout`.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// After refusing a request, how long to keep reading what the client is
/// still sending, and how much of it, before closing.
const LINGER: Duration = Duration::from_secs(1);
const LINGER_BYTES: u64 = 1024 * 1024;

/// Where to listen, which database to use, and how many worker threads to
/// run: Puma's threads, each with its own connection.
pub struct Config {
    pub address: String,
    pub database_url: String,
    pub workers: usize,
}

pub struct Running {
    pub address: SocketAddr,
    stopping: Arc<AtomicBool>,
    jobs: Sender<Job>,
    intake: JoinHandle<()>,
    workers: Vec<JoinHandle<()>>,
}

/// A request read in full.
struct Incoming {
    method: String,
    target: String,
    content_type: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A request and where its response goes, or a signal to stop.
enum Job {
    Serve(Incoming, Sender<Response>),
    Stop,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Starts listening and returns once the workers are running. Each
/// connection gets a thread that reads requests off it in full, so a client
/// that stalls mid-upload holds that thread, never a database worker;
/// workers only see requests that are fully read.
pub fn start(router: Router, config: Config) -> Result<Running, BoxError> {
    let listener = TcpListener::bind(config.address.as_str())?;
    let address = listener.local_addr()?;
    let (jobs, queue) = channel::<Job>();
    let queue = Arc::new(Mutex::new(queue));
    let router = Arc::new(router);
    let workers = (0..config.workers.max(1))
        .map(|_| {
            let (queue, router, url) = (queue.clone(), router.clone(), config.database_url.clone());
            std::thread::spawn(move || work(&queue, &router, &url))
        })
        .collect();
    let stopping = Arc::new(AtomicBool::new(false));
    let intake = {
        let (stopping, jobs) = (stopping.clone(), jobs.clone());
        std::thread::spawn(move || accept(&listener, &jobs, &stopping))
    };
    Ok(Running { address, stopping, jobs, intake, workers })
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
    /// returns. Connection threads end on their own when their client goes
    /// quiet.
    pub fn stop(self) {
        self.stopping.store(true, Ordering::SeqCst);
        // `accept` only looks at the flag when a connection arrives.
        TcpStream::connect(self.address).ok();
        for _ in &self.workers {
            self.jobs.send(Job::Stop).ok();
        }
        self.join();
    }
}

fn accept(listener: &TcpListener, jobs: &Sender<Job>, stopping: &AtomicBool) {
    for stream in listener.incoming() {
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        match stream {
            Ok(stream) => {
                let jobs = jobs.clone();
                std::thread::spawn(move || serve(stream, &jobs));
            }
            // Most likely out of file descriptors; don't spin on it.
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Reads requests off one connection and writes their responses in order,
/// until the client closes it, goes quiet, or asks to close.
fn serve(stream: TcpStream, jobs: &Sender<Job>) {
    // Each response goes out in one write, and TCP_NODELAY sends it now
    // rather than holding its last segment for the client's delayed ACK.
    let ready = stream
        .set_nodelay(true)
        .and_then(|_| stream.set_read_timeout(Some(IDLE_TIMEOUT)))
        .and_then(|_| stream.set_write_timeout(Some(IDLE_TIMEOUT)));
    let (Ok(()), Ok(read_half)) = (ready, stream.try_clone()) else { return };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    loop {
        let request = wire::read_head(&mut reader).and_then(|head| {
            let body = wire::read_body(&head, &mut reader, &mut writer, MAX_BODY_BYTES)?;
            Ok((head, body))
        });
        let (head, body) = match request {
            Ok(request) => request,
            Err(WireError::Refuse(status)) => return refuse(&mut reader, &mut writer, status),
            Err(WireError::Gone) => return,
        };
        let close = !head.keep_alive();
        let head_only = head.method == "HEAD";
        let content_type = head.header("Content-Type").map(str::to_string);
        let incoming = Incoming { method: head.method, target: head.target, content_type, headers: head.headers, body };
        let (reply, response) = channel();
        if jobs.send(Job::Serve(incoming, reply)).is_err() {
            return;
        }
        let Ok(response) = response.recv() else { return };
        if wire::write_response(&mut writer, &response, head_only, close).is_err() || close {
            return;
        }
    }
}

/// Answers with an error page and closes. Closing a socket with request
/// bytes still unread makes the kernel send a reset, which can destroy the
/// response before the client reads it, so read and discard for a moment
/// first.
fn refuse(reader: &mut impl Read, writer: &mut TcpStream, status: u16) {
    if wire::write_response(writer, &error_page(status), false, true).is_err() {
        return;
    }
    writer.shutdown(Shutdown::Write).ok();
    writer.set_read_timeout(Some(LINGER)).ok();
    io::copy(&mut reader.take(LINGER_BYTES), &mut io::sink()).ok();
}

fn work(queue: &Mutex<Receiver<Job>>, router: &Router, url: &str) {
    let mut client: Option<Client> = None;
    loop {
        let job = queue.lock().map(|queue| queue.recv());
        let Ok(Ok(Job::Serve(incoming, reply))) = job else { return };
        // A connection the database closed (restart, failover, idle kill)
        // is replaced rather than reused.
        let connection = match client.take().filter(|c| !c.is_closed()) {
            Some(client) => Ok(client),
            None => Client::connect(url, NoTls),
        };
        let response = match connection {
            Ok(connection) => {
                let (response, kept) = handle(router, connection, &incoming);
                client = kept.filter(|c| !c.is_closed());
                response
            }
            Err(error) => {
                eprintln!("database connection failed: {error}");
                error_page(500)
            }
        };
        reply.send(response).ok();
    }
}

/// Runs one request in a fresh `Ctx`. A panic becomes a 500 and costs the
/// connection, since the panic may have left it mid-transaction.
fn handle(router: &Router, client: Client, incoming: &Incoming) -> (Response, Option<Client>) {
    let mut req = build_request(Ctx::new(client), incoming);
    match catch_unwind(AssertUnwindSafe(|| router.call(&mut req))) {
        Ok(response) => (response, Some(req.ctx.into_client())),
        Err(_) => (error_page(500), None),
    }
}

fn build_request(ctx: Ctx, incoming: &Incoming) -> Request {
    let target = incoming.target.as_str();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Request::new(ctx, &incoming.method, path)
        .with_query(query)
        .with_headers(incoming.headers.clone())
        .with_body(incoming.content_type.as_deref(), &incoming.body)
}

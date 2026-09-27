use std::io::{self, BufRead, BufReader};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::limits::Timed;
use super::wire::{self, WireError};
use super::{Limits, Request, Response, Router, error_page};
use crate::{Connection, Ctx};

/// After refusing a request, how long to keep reading what the client is
/// still sending, and how much of it, before closing.
const LINGER: Duration = Duration::from_secs(1);
const LINGER_BYTES: u64 = 1024 * 1024;

/// Where to listen, which database to use, how many worker threads to run
/// (Puma's threads, each with its own connection), and what one client may
/// hold of the server.
pub struct Config {
    pub address: String,
    pub database_url: String,
    pub workers: usize,
    pub limits: Limits,
    /// `SECRET_KEY_BASE`, for the session cookie.
    pub secret_key_base: Option<String>,
    /// `REDIS_URL`, where `perform_later` puts jobs.
    pub redis_url: Option<String>,
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
    let router = router.secret_key_base(config.secret_key_base.as_deref());
    // Rails won't boot without the secret its session cookie needs.
    if router.needs_secret() {
        return Err("SECRET_KEY_BASE is not set, and the app's session cookie needs it".into());
    }
    let router = Arc::new(router);
    // Only an app with jobs names its Redis.
    if let Some(url) = &config.redis_url {
        crate::jobs::configure(Some(url))?;
    }
    let workers = (0..config.workers.max(1))
        .map(|_| {
            let (queue, router, url) = (queue.clone(), router.clone(), config.database_url.clone());
            std::thread::spawn(move || work(&queue, &router, &url))
        })
        .collect();
    let stopping = Arc::new(AtomicBool::new(false));
    let intake = {
        let (stopping, jobs, limits) = (stopping.clone(), jobs.clone(), Arc::new(config.limits));
        std::thread::spawn(move || accept(&listener, &jobs, &stopping, &limits))
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

fn accept(listener: &TcpListener, jobs: &Sender<Job>, stopping: &AtomicBool, limits: &Arc<Limits>) {
    let open = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        match stream {
            Ok(stream) if open.load(Ordering::SeqCst) >= limits.max_connections => busy(stream),
            Ok(stream) => {
                let slot = Slot::take(&open);
                let (jobs, limits) = (jobs.clone(), limits.clone());
                // Out of threads, the connection is dropped, and its slot
                // with it; a panic here would end the accept loop for good.
                let spawned = std::thread::Builder::new().spawn(move || {
                    let _slot = slot;
                    serve(stream, &jobs, &limits);
                });
                if spawned.is_err() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            // Most likely out of file descriptors; don't spin on it.
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// One of `max_connections`, given back when its connection ends, however
/// it ends.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(open: &Arc<AtomicUsize>) -> Self {
        open.fetch_add(1, Ordering::SeqCst);
        Self(open.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A connection past `max_connections`: a 503, and closed, all without
/// waiting on the client. Whatever it already sent is read first, since
/// closing with it unread makes the kernel reset the connection, which
/// can destroy the 503 before the client reads it.
fn busy(mut stream: TcpStream) {
    stream.set_write_timeout(Some(Duration::from_millis(100))).ok();
    wire::write_response(&mut stream, &error_page(503), false, true).ok();
    stream.shutdown(Shutdown::Write).ok();
    if stream.set_nonblocking(true).is_ok() {
        let mut sink = [0u8; 4096];
        for _ in 0..16 {
            if !matches!(io::Read::read(&mut stream, &mut sink), Ok(n) if n > 0) {
                break;
            }
        }
    }
}

/// Reads requests off one connection and writes their responses in order,
/// until the client closes it, goes quiet, asks to close, or runs past a
/// limit.
fn serve(stream: TcpStream, jobs: &Sender<Job>, limits: &Limits) {
    // Each response goes out in one write, and TCP_NODELAY sends it now
    // rather than holding its last segment for the client's delayed ACK.
    if stream.set_nodelay(true).is_err() {
        return;
    }
    // Reads and writes share the one socket (and file descriptor); their
    // timeouts are separate socket options.
    let mut reader = BufReader::new(Timed::new(&stream, limits.idle_timeout));
    let mut writer = Timed::new(&stream, limits.write_timeout);
    loop {
        // Waiting for the next request is idle time; its first byte starts
        // the clock on its headers, and the headers' end on its body.
        reader.get_mut().within(None);
        if !matches!(reader.fill_buf(), Ok(bytes) if !bytes.is_empty()) {
            return;
        }
        reader.get_mut().within(Some(limits.header_timeout));
        let request = wire::read_head(&mut reader).and_then(|head| {
            reader.get_mut().within_at(limits.body_timeout, limits.min_rate);
            writer.within(Some(limits.body_timeout));
            let body = wire::read_body(&head, &mut reader, &mut writer, limits.max_body_bytes)?;
            Ok((head, body))
        });
        let (head, body) = match request {
            Ok(request) => request,
            Err(WireError::Refuse(status)) => return refuse(&mut reader, &mut writer, status, limits),
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
        writer.within_at(limits.write_timeout, limits.min_rate);
        if wire::write_response(&mut writer, &response, head_only, close).is_err() || close {
            return;
        }
    }
}

/// Answers with an error page and closes. Closing a socket with request
/// bytes still unread makes the kernel send a reset, which can destroy the
/// response before the client reads it, so read and discard for a moment
/// first.
fn refuse(reader: &mut BufReader<Timed<'_>>, writer: &mut Timed<'_>, status: u16, limits: &Limits) {
    writer.within(Some(limits.write_timeout));
    if wire::write_response(writer, &error_page(status), false, true).is_err() {
        return;
    }
    writer.get_ref().shutdown(Shutdown::Write).ok();
    reader.get_mut().within(Some(LINGER));
    io::copy(&mut io::Read::take(reader, LINGER_BYTES), &mut io::sink()).ok();
}

fn work(queue: &Mutex<Receiver<Job>>, router: &Router, url: &str) {
    let mut kept: Option<Connection> = None;
    loop {
        let job = queue.lock().map(|queue| queue.recv());
        let Ok(Ok(Job::Serve(incoming, reply))) = job else { return };
        // A connection the database closed (restart, failover, idle kill)
        // is replaced rather than reused, and its statements with it.
        let connection = match kept.take().filter(|c| !c.is_closed()) {
            Some(connection) => Ok(connection),
            None => Connection::connect(url),
        };
        let response = match connection {
            Ok(connection) => {
                let (response, back) = handle(router, connection, &incoming);
                kept = back.filter(|c| !c.is_closed());
                response
            }
            Err(error) => {
                eprintln!("database connection failed: {error}");
                router.failure(
                    super::request::wants_json_errors_for(&incoming.target, &incoming.headers, incoming.content_type.as_deref(), &incoming.body),
                    500,
                )
            }
        };
        reply.send(response).ok();
    }
}

/// Runs one request in a fresh `Ctx`. A panic becomes a 500. It costs the
/// connection only when it left a transaction open, so a request that
/// overflows an integer on purpose doesn't make the next one reconnect.
fn handle(router: &Router, connection: Connection, incoming: &Incoming) -> (Response, Option<Connection>) {
    let mut req = build_request(Ctx::resume(connection), incoming);
    match catch_unwind(AssertUnwindSafe(|| router.call(&mut req))) {
        Ok(response) => (response, Some(req.ctx.into_connection())),
        Err(_) => {
            let response = router.failure(req.wants_json_errors(), 500);
            (response, (req.ctx.depth == 0).then(|| req.ctx.into_connection()))
        }
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

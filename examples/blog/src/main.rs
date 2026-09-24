use rustonrails::server::{self, Config};

/// Serves the blog: `DATABASE_URL`, `BIND` (default 127.0.0.1:3000) and
/// `WORKERS` (default 5, like Puma's threads).
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config {
        address: std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:3000".into()),
        database_url: std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?,
        workers: std::env::var("WORKERS").ok().and_then(|w| w.parse().ok()).unwrap_or(5),
    };
    let running = server::start(blog::routes::routes(), config)?;
    eprintln!("blog listening on {}", running.address);
    running.join();
    Ok(())
}

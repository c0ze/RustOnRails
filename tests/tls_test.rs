//! TLS to Postgres, against a server that has it. Ignored unless asked for:
//!
//!     RUSTONRAILS_TLS_TEST_HOST=localhost:54349 RUSTONRAILS_TLS_TEST_CA=/path/ca.pem \
//!         cargo test --test tls_test -- --ignored
//!
//! The server's certificate must be signed by that CA for the name
//! `localhost` only, so that 127.0.0.1 is the wrong host name for it.

use rustonrails::{Ctx, Error};

fn server() -> (String, String) {
    let host = std::env::var("RUSTONRAILS_TLS_TEST_HOST").expect("RUSTONRAILS_TLS_TEST_HOST, e.g. localhost:54349");
    let ca = std::env::var("RUSTONRAILS_TLS_TEST_CA").expect("RUSTONRAILS_TLS_TEST_CA, the CA's PEM file");
    (host, ca)
}

/// Whether the session is encrypted, as the server sees it.
fn encrypted(url: &str) -> Result<bool, Error> {
    let mut ctx = Ctx::connect(url)?;
    let rows = ctx.query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()", &[])?;
    Ok(rows[0].get(0))
}

#[test]
#[ignore = "needs a Postgres with TLS: RUSTONRAILS_TLS_TEST_HOST and RUSTONRAILS_TLS_TEST_CA"]
fn test_sslmode_as_libpq_reads_it() {
    let (host, ca) = server();
    let (_, port) = host.rsplit_once(':').expect("host:port");
    let url = |host: &str, query: &str| format!("postgres://postgres@{host}/postgres?{query}");
    let by_ip = format!("127.0.0.1:{port}");

    // libpq's default, prefer, and require: encrypted, nothing verified.
    assert!(encrypted(&url(&host, "application_name=default")).unwrap());
    assert!(encrypted(&url(&by_ip, "sslmode=require")).unwrap());
    assert!(!encrypted(&url(&host, "sslmode=disable")).unwrap());
    // verify-full checks the chain and the name; verify-ca only the chain.
    assert!(encrypted(&url(&host, &format!("sslmode=verify-full&sslrootcert={ca}"))).unwrap());
    assert!(encrypted(&url(&by_ip, &format!("sslmode=verify-ca&sslrootcert={ca}"))).unwrap());
    assert!(encrypted(&url(&by_ip, &format!("sslmode=verify-full&sslrootcert={ca}"))).is_err());
    // A CA the system doesn't trust, and no sslrootcert: refused.
    assert!(encrypted(&url(&host, "sslmode=verify-full")).is_err());
    // require with a root certificate verifies the chain, as libpq does.
    assert!(encrypted(&url(&by_ip, &format!("sslmode=require&sslrootcert={ca}"))).unwrap());
    // A key=value connection string reads the same.
    let (name, port) = host.rsplit_once(':').unwrap();
    let string = format!("host={name} port={port} user=postgres dbname=postgres sslmode=verify-full sslrootcert='{ca}'");
    assert!(encrypted(&string).unwrap());
}

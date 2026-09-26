//! Opening a database connection, with TLS as libpq (and so Rails' `pg`)
//! reads the URL.

use std::sync::LazyLock;

use native_tls::{Certificate, TlsConnector};
use postgres::Client;
use postgres::config::SslMode;
use postgres_native_tls::MakeTlsConnector;
use regex::Regex;

use crate::{Error, Result};

/// libpq's `sslmode`, which the driver knows only part of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Disable,
    /// TLS when the server offers it, unverified: libpq's default.
    Prefer,
    /// TLS, unverified unless `sslrootcert` names a CA.
    Require,
    /// TLS with a certificate signed by a trusted CA.
    VerifyCa,
    /// That, for this host name too.
    VerifyFull,
}

/// The TLS settings in a database URL.
#[derive(Debug, PartialEq)]
struct Tls {
    mode: Mode,
    root_cert: Option<String>,
}

/// Connects to `url`, which may name any `sslmode` libpq knows
/// (`disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`)
/// and an `sslrootcert` file of CA certificates in PEM.
pub(crate) fn connect(url: &str) -> Result<Client> {
    let (url, tls) = take_tls(url)?;
    let mut config: postgres::Config = url.parse()?;
    config.ssl_mode(match tls.mode {
        Mode::Disable => SslMode::Disable,
        Mode::Prefer => SslMode::Prefer,
        _ => SslMode::Require,
    });
    Ok(config.connect(connector(&tls)?)?)
}

/// As libpq does: `require` verifies nothing unless there's a root
/// certificate to verify against, `verify-ca` checks the chain and
/// `verify-full` the host name as well.
fn connector(tls: &Tls) -> Result<MakeTlsConnector> {
    let mut builder = TlsConnector::builder();
    if let Some(path) = &tls.root_cert {
        let pem = std::fs::read(path).map_err(|e| Error::Connect(format!("can't read sslrootcert {path}: {e}")))?;
        let certificate = Certificate::from_pem(&pem).map_err(|e| Error::Connect(format!("sslrootcert {path}: {e}")))?;
        builder.add_root_certificate(certificate);
    }
    let chain = match tls.mode {
        Mode::VerifyCa | Mode::VerifyFull => true,
        Mode::Require => tls.root_cert.is_some(),
        Mode::Disable | Mode::Prefer => false,
    };
    builder.danger_accept_invalid_certs(!chain);
    builder.danger_accept_invalid_hostnames(tls.mode != Mode::VerifyFull);
    let connector = builder.build().map_err(|e| Error::Connect(format!("TLS setup failed: {e}")))?;
    Ok(MakeTlsConnector::new(connector))
}

/// `key = value` in a libpq connection string, the value quoted or not.
static SETTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)(sslmode|sslrootcert)\s*=\s*('(?:[^'\\]|\\.)*'|[^\s']+)").expect("regex"));

/// Takes `sslmode` and `sslrootcert` out of a URL or a `key=value`
/// connection string, leaving the rest for the driver, which refuses
/// `verify-ca`, `verify-full` and `sslrootcert`.
fn take_tls(url: &str) -> Result<(String, Tls)> {
    let mut mode = None;
    let mut root_cert = None;
    let mut keep = |key: &str, value: String| match key {
        "sslmode" => mode = Some(value),
        _ => root_cert = Some(value),
    };
    let rest = if url.contains("://") {
        match url.split_once('?') {
            Some((base, query)) => {
                let mut kept = Vec::new();
                for pair in query.split('&') {
                    match form_urlencoded::parse(pair.as_bytes()).next() {
                        Some((key, value)) if key == "sslmode" || key == "sslrootcert" => keep(&key, value.into_owned()),
                        _ => kept.push(pair),
                    }
                }
                if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
            }
            None => url.to_string(),
        }
    } else {
        for found in SETTING.captures_iter(url) {
            let value = &found[2];
            let value = match value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
                Some(quoted) => quoted.replace("\\'", "'").replace("\\\\", "\\"),
                None => value.to_string(),
            };
            keep(&found[1], value);
        }
        SETTING.replace_all(url, "").trim().to_string()
    };
    let mode = match mode.as_deref() {
        None | Some("prefer" | "allow") => Mode::Prefer,
        Some("disable") => Mode::Disable,
        Some("require") => Mode::Require,
        Some("verify-ca") => Mode::VerifyCa,
        Some("verify-full") => Mode::VerifyFull,
        Some(other) => return Err(Error::Connect(format!("sslmode {other:?} isn't one libpq knows"))),
    };
    Ok((rest, Tls { mode, root_cert }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(url: &str) -> (String, Mode, Option<String>) {
        let (rest, tls) = take_tls(url).unwrap();
        (rest, tls.mode, tls.root_cert)
    }

    #[test]
    fn test_tls_settings_come_out_of_a_url() {
        assert_eq!(("postgres://u@h/db".into(), Mode::Prefer, None), take("postgres://u@h/db"));
        assert_eq!(
            ("postgres://u@h/db?application_name=app".into(), Mode::VerifyFull, Some("/etc/ssl/root ca.pem".into())),
            take("postgres://u@h/db?sslmode=verify-full&application_name=app&sslrootcert=%2Fetc%2Fssl%2Froot%20ca.pem")
        );
        assert_eq!(("postgresql://h/db".into(), Mode::Require, None), take("postgresql://h/db?sslmode=require"));
        assert_eq!(("postgres://h/db".into(), Mode::Prefer, None), take("postgres://h/db?sslmode=allow"));
    }

    #[test]
    fn test_tls_settings_come_out_of_a_connection_string() {
        assert_eq!(
            ("host=h dbname=db".into(), Mode::VerifyCa, Some("/certs/it's.pem".into())),
            take("host=h sslmode = verify-ca dbname=db sslrootcert='/certs/it\\'s.pem'")
        );
        assert_eq!(("host=h".into(), Mode::Disable, None), take("sslmode=disable host=h"));
    }

    #[test]
    fn test_an_unknown_mode_or_a_missing_root_certificate_is_an_error() {
        assert!(matches!(take_tls("postgres://h/db?sslmode=always"), Err(Error::Connect(_))));
        let tls = Tls { mode: Mode::VerifyFull, root_cert: Some("/nonexistent/root.pem".into()) };
        assert!(matches!(connector(&tls), Err(Error::Connect(message)) if message.contains("/nonexistent/root.pem")));
    }
}

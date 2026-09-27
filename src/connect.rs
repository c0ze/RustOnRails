//! Opening a database connection, with TLS as libpq (and so Rails' `pg`)
//! reads the URL.

use std::path::{Path, PathBuf};

use native_tls::{Certificate, TlsConnector};
use postgres::Client;
use postgres::config::SslMode;
use postgres_native_tls::MakeTlsConnector;

use crate::{Error, Result};

/// libpq's `sslmode`, which the driver knows only part of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Disable,
    /// TLS when the server offers it, unverified: libpq's default.
    Prefer,
    /// TLS, unverified unless there's a root certificate to verify with.
    Require,
    /// TLS with a certificate a trusted CA signed.
    VerifyCa,
    /// That, for this host name too.
    VerifyFull,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Disable => "disable",
            Mode::Prefer => "prefer",
            Mode::Require => "require",
            Mode::VerifyCa => "verify-ca",
            Mode::VerifyFull => "verify-full",
        }
    }
}

/// The TLS settings in a database URL.
#[derive(Debug, PartialEq)]
struct Tls {
    mode: Option<Mode>,
    root_cert: Option<String>,
}

/// Whose certificates a verified connection trusts.
#[derive(Debug, PartialEq)]
enum Roots {
    /// Nothing to verify with.
    None,
    /// Only the CAs in this PEM file: `sslrootcert`, or libpq's default
    /// `~/.postgresql/root.crt`.
    File(PathBuf),
    /// The operating system's, asked for with `sslrootcert=system`.
    System,
}

/// Connects to `url`, which may name any `sslmode` libpq knows
/// (`disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`)
/// and an `sslrootcert`: a PEM file of CA certificates, or `system`.
pub(crate) fn connect(url: &str) -> Result<Client> {
    connect_to(url, None)
}

/// `connect`, to the database `dbname` rather than the URL's: the same
/// host, user and TLS settings.
pub(crate) fn connect_to(url: &str, dbname: Option<&str>) -> Result<Client> {
    let (url, mut tls) = take_tls(url)?;
    let mut config: postgres::Config = url.parse()?;
    if let Some(dbname) = dbname {
        config.dbname(dbname);
    }
    adopt(&mut tls, config.get_ssl_mode());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (mode, roots) = settle(&tls, home.as_deref())?;
    config.ssl_mode(match mode {
        Mode::Disable => SslMode::Disable,
        Mode::Prefer => SslMode::Prefer,
        _ => SslMode::Require,
    });
    Ok(config.connect(connector(mode, &roots)?)?)
}

/// The database `url` names, as the driver reads it: in a URL, the path
/// before any query.
pub(crate) fn database_name(url: &str) -> Result<Option<String>> {
    let (url, _) = take_tls(url)?;
    let config: postgres::Config = url.parse()?;
    Ok(config.get_dbname().map(str::to_string))
}

/// An `sslmode` that got past `take_tls` is still in the URL the driver
/// parsed, so the driver's reading of it becomes this one's: the TLS
/// checks then follow the mode the driver will use. (A verify mode the
/// driver doesn't know fails its parse instead.)
fn adopt(tls: &mut Tls, parsed: SslMode) {
    if tls.mode.is_none() {
        tls.mode = match parsed {
            SslMode::Disable => Some(Mode::Disable),
            SslMode::Require => Some(Mode::Require),
            _ => None,
        };
    }
}

/// The mode and roots libpq would use. `verify-ca` and `verify-full` need
/// something to verify with, or they fail rather than trust anything;
/// `sslrootcert=system` goes only with `verify-full`, which it makes the
/// default, since any public CA can sign a certificate for some name.
fn settle(tls: &Tls, home: Option<&Path>) -> Result<(Mode, Roots)> {
    let roots = match tls.root_cert.as_deref() {
        Some("system") => Roots::System,
        Some(path) => Roots::File(PathBuf::from(path)),
        None => home
            .map(|home| home.join(".postgresql/root.crt"))
            .filter(|default| default.is_file())
            .map_or(Roots::None, Roots::File),
    };
    let mode = match (tls.mode, &roots) {
        (None, Roots::System) => Mode::VerifyFull,
        (None, _) => Mode::Prefer,
        (Some(mode), _) => mode,
    };
    match (mode, &roots) {
        (Mode::VerifyFull, Roots::System) => {}
        (_, Roots::System) => {
            return Err(Error::Connect(format!("sslmode {} is too weak for sslrootcert=system; use verify-full", mode.name())));
        }
        (Mode::VerifyCa | Mode::VerifyFull, Roots::None) => {
            return Err(Error::Connect(format!(
                "sslmode {} needs a root certificate: set sslrootcert to a PEM file, provide ~/.postgresql/root.crt, \
                 or use sslrootcert=system with verify-full",
                mode.name()
            )));
        }
        _ => {}
    }
    Ok((mode, roots))
}

/// As libpq does: `prefer` verifies nothing, `require` verifies the chain
/// when there's a root file, `verify-ca` always does, and `verify-full`
/// checks the host name as well. A root file replaces the system's CAs,
/// and is read only when something is verified against it.
fn connector(mode: Mode, roots: &Roots) -> Result<MakeTlsConnector> {
    let chain = match mode {
        Mode::VerifyCa | Mode::VerifyFull => true,
        Mode::Require => matches!(roots, Roots::File(_)),
        Mode::Disable | Mode::Prefer => false,
    };
    let mut builder = TlsConnector::builder();
    if let (true, Roots::File(path)) = (chain, roots) {
        let shown = path.display();
        let pem = std::fs::read(path).map_err(|e| Error::Connect(format!("can't read sslrootcert {shown}: {e}")))?;
        let certificates = Certificate::stack_from_pem(&pem).map_err(|e| Error::Connect(format!("sslrootcert {shown}: {e}")))?;
        if certificates.is_empty() {
            return Err(Error::Connect(format!("sslrootcert {shown} holds no certificates")));
        }
        builder.disable_built_in_roots(true);
        for certificate in certificates {
            builder.add_root_certificate(certificate);
        }
    }
    builder.danger_accept_invalid_certs(!chain);
    builder.danger_accept_invalid_hostnames(mode != Mode::VerifyFull);
    let connector = builder.build().map_err(|e| Error::Connect(format!("TLS setup failed: {e}")))?;
    Ok(MakeTlsConnector::new(connector))
}

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
    // libpq's two URI prefixes; anything else is a key=value string.
    let rest = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        // The driver reads up to the first `@` anywhere as the user and
        // password. An `@` after the host's first `/` or `?` would make it
        // send part of the path or query as the user name and drop the
        // query's sslmode, so such a URL is refused: the user name, password
        // and query must spell `@`, `/` and `?` percent-encoded.
        let (scheme, after) = url.split_once("://").expect("a URL prefix");
        let host_end = after.find(['/', '?']).unwrap_or(after.len());
        if after.find('@').is_some_and(|at| at > host_end) {
            return Err(Error::Connect(
                "the database URL has an @ after its host; write @, / and ? in the user name, password and query \
                 as %40, %2F and %3F"
                    .to_string(),
            ));
        }
        let credentials = scheme.len() + 3 + after[..host_end].find('@').map_or(0, |at| at + 1);
        match url[credentials..].find('?').map(|at| url.split_at(credentials + at)) {
            Some((base, query)) => {
                let query = &query[1..];
                let mut kept = Vec::new();
                for pair in query.split('&') {
                    // Percent-decoded as the driver does it: `+` is a plus.
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
                    match decode(key).as_str() {
                        key @ ("sslmode" | "sslrootcert") => keep(key, decode(value)),
                        _ => kept.push(pair),
                    }
                }
                if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
            }
            None => url.to_string(),
        }
    } else {
        let mut kept = Vec::new();
        for setting in settings(url)? {
            match setting.key {
                "sslmode" | "sslrootcert" => keep(setting.key, setting.value),
                _ => kept.push(setting.raw),
            }
        }
        kept.join(" ")
    };
    let mode = match mode.as_deref() {
        None => None,
        Some("prefer" | "allow") => Some(Mode::Prefer),
        Some("disable") => Some(Mode::Disable),
        Some("require") => Some(Mode::Require),
        Some("verify-ca") => Some(Mode::VerifyCa),
        Some("verify-full") => Some(Mode::VerifyFull),
        Some(other) => return Err(Error::Connect(format!("sslmode {other:?} isn't one libpq knows"))),
    };
    Ok((rest, Tls { mode, root_cert }))
}

/// One `key = value` of a libpq connection string, and its text as given.
struct Setting<'a> {
    key: &'a str,
    value: String,
    raw: &'a str,
}

/// A libpq connection string's settings, in order: a value is a single
/// word or a quoted string, and a backslash in either takes the next
/// character as it is, so `application_name='x sslmode=disable'` and
/// `application_name=x\ sslmode=disable` are each one setting, not two.
fn settings(string: &str) -> Result<Vec<Setting<'_>>> {
    let malformed = || Error::Connect("the database connection string isn't `key=value` pairs".to_string());
    let bytes = string.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() {
            return Ok(found);
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &string[start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if key.is_empty() || bytes.get(i) != Some(&b'=') {
            return Err(malformed());
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if bytes.get(i) == Some(&b'\'') {
            i += 1;
            loop {
                match bytes.get(i) {
                    None => return Err(malformed()),
                    Some(b'\'') => break,
                    Some(b'\\') if i + 1 < bytes.len() => {
                        let c = string[i + 1..].chars().next().expect("inside the string");
                        value.push(c);
                        i += 1 + c.len_utf8();
                        continue;
                    }
                    Some(_) => {
                        let c = string[i..].chars().next().expect("inside the string");
                        value.push(c);
                        i += c.len_utf8();
                        continue;
                    }
                }
            }
            i += 1;
        } else {
            while let Some(c) = string[i..].chars().next().filter(|c| !c.is_whitespace()) {
                i += c.len_utf8();
                if c == '\\'
                    && let Some(next) = string[i..].chars().next()
                {
                    value.push(next);
                    i += next.len_utf8();
                } else {
                    value.push(c);
                }
            }
        }
        found.push(Setting { key, value, raw: &string[start..i] });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(url: &str) -> (String, Option<Mode>, Option<String>) {
        let (rest, tls) = take_tls(url).unwrap();
        (rest, tls.mode, tls.root_cert)
    }

    #[test]
    fn test_tls_settings_come_out_of_a_url() {
        assert_eq!(("postgres://u@h/db".into(), None, None), take("postgres://u@h/db"));
        assert_eq!(
            ("postgres://u@h/db?application_name=app".into(), Some(Mode::VerifyFull), Some("/etc/ssl/root ca.pem".into())),
            take("postgres://u@h/db?sslmode=verify-full&application_name=app&sslrootcert=%2Fetc%2Fssl%2Froot%20ca.pem")
        );
        assert_eq!(("postgresql://h/db".into(), Some(Mode::Require), None), take("postgresql://h/db?sslmode=require"));
        assert_eq!(("postgres://h/db".into(), Some(Mode::Prefer), None), take("postgres://h/db?sslmode=allow"));
        // Percent-decoding only: a `+` stays a plus.
        assert_eq!(Some("/certs/ca+2025.pem".into()), take("postgres://h/db?sslrootcert=/certs/ca+2025.pem").2);
    }

    #[test]
    fn test_the_database_name_is_the_path_before_the_query() {
        let url = "postgres://u@h/ror_test?sslmode=verify-full&sslrootcert=system";
        assert_eq!(Some("ror_test".to_string()), database_name(url).unwrap());
        assert_eq!(Some("db".to_string()), database_name("host=h dbname=db sslmode=require").unwrap());
    }

    #[test]
    fn test_tls_settings_come_out_of_a_connection_string() {
        assert_eq!(
            ("host=h dbname=db".into(), Some(Mode::VerifyCa), Some("/certs/it's.pem".into())),
            take("host=h sslmode = verify-ca dbname=db sslrootcert='/certs/it\\'s.pem'")
        );
        assert_eq!(("host=h".into(), Some(Mode::Disable), None), take("sslmode=disable host=h"));
        // A setting inside another's quoted value is part of that value.
        assert_eq!(
            ("host=h application_name='prod sslmode=disable'".into(), Some(Mode::VerifyFull), None),
            take("host=h sslmode=verify-full application_name='prod sslmode=disable'")
        );
        // So is one behind a backslash-escaped space, as libpq reads it.
        assert_eq!(
            ("host=h application_name=x\\ sslmode=disable".into(), Some(Mode::VerifyFull), None),
            take("host=h sslmode=verify-full application_name=x\\ sslmode=disable")
        );
        // `://` in a value doesn't make a connection string a URL.
        assert_eq!(("host=h password=x://y".into(), Some(Mode::Require), None), take("host=h password=x://y sslmode=require"));
        assert!(matches!(take_tls("host=h application_name='unterminated"), Err(Error::Connect(_))));
        assert!(matches!(take_tls("host=h =x"), Err(Error::Connect(_))));
    }

    /// The query starts after the credentials; a `?` in a password has to
    /// be %3F, since unencoded it can't be told from the query's start.
    #[test]
    fn test_the_query_starts_after_the_credentials() {
        let (rest, mode, root) = take("postgres://app:pa%3Fss@db/prod?sslmode=require&sslrootcert=/ca.pem");
        assert_eq!(("postgres://app:pa%3Fss@db/prod".to_string(), Some(Mode::Require), Some("/ca.pem".to_string())), (rest, mode, root));
        assert!(matches!(take_tls("postgres://app:pa?ss@db/prod?sslmode=require"), Err(Error::Connect(_))));
    }

    /// An `@` after the host's `/` or `?` would be credentials to the
    /// driver: refused, so the sslmode after it can't be lost.
    #[test]
    fn test_an_at_sign_after_the_host_is_refused() {
        let url = "postgres://db.example.com/prod?sslmode=verify-full&sslrootcert=/ca.pem&application_name=ops@web-1";
        assert!(matches!(take_tls(url), Err(Error::Connect(message)) if message.contains("%40")));
        // With no path, the host ends at the `?`.
        assert!(take_tls("postgres://db.example.com?sslmode=require&application_name=ops@web-1").is_err());
        assert!(take_tls("postgres://u:pa/ss@db/prod").is_err());
        assert!(take_tls("postgres://db.example.com/prod?application_name=ops%40web-1&sslmode=require").is_ok());
    }

    /// Whatever sslmode the driver found and this didn't becomes this one's,
    /// so require with a root file verifies, as libpq does.
    #[test]
    fn test_the_drivers_mode_is_adopted_when_this_found_none() {
        let mut missed = tls(None, Some("/ca.pem"));
        adopt(&mut missed, SslMode::Require);
        assert_eq!(Some(Mode::Require), missed.mode);
        let mut found = tls(Some(Mode::VerifyFull), None);
        adopt(&mut found, SslMode::Disable);
        assert_eq!(Some(Mode::VerifyFull), found.mode);
        let mut default = tls(None, Some("system"));
        adopt(&mut default, SslMode::Prefer);
        assert_eq!((Mode::VerifyFull, Roots::System), settle(&default, None).unwrap());
    }

    fn tls(mode: Option<Mode>, root_cert: Option<&str>) -> Tls {
        Tls { mode, root_cert: root_cert.map(String::from) }
    }

    /// As libpq 16 decides: psql refuses the same combinations.
    #[test]
    fn test_modes_and_roots_as_libpq_settles_them() {
        let nowhere = Some(Path::new("/nonexistent-home"));
        assert_eq!((Mode::Prefer, Roots::None), settle(&tls(None, None), nowhere).unwrap());
        assert_eq!((Mode::VerifyFull, Roots::System), settle(&tls(None, Some("system")), nowhere).unwrap());
        assert!(settle(&tls(Some(Mode::Require), Some("system")), nowhere).is_err());
        assert!(settle(&tls(Some(Mode::VerifyCa), Some("system")), nowhere).is_err());
        assert!(settle(&tls(Some(Mode::VerifyCa), None), nowhere).is_err());
        assert!(settle(&tls(Some(Mode::VerifyFull), None), nowhere).is_err());
        let file = settle(&tls(Some(Mode::VerifyCa), Some("/ca.pem")), nowhere).unwrap();
        assert_eq!((Mode::VerifyCa, Roots::File("/ca.pem".into())), file);
    }

    #[test]
    fn test_libpqs_default_root_file_counts() {
        let home = std::env::temp_dir().join(format!("rustonrails-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".postgresql")).unwrap();
        std::fs::write(home.join(".postgresql/root.crt"), "").unwrap();
        let settled = settle(&tls(Some(Mode::VerifyFull), None), Some(&home)).unwrap();
        assert_eq!((Mode::VerifyFull, Roots::File(home.join(".postgresql/root.crt"))), settled);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn test_a_root_file_that_is_missing_or_empty_is_an_error() {
        let missing = connector(Mode::VerifyFull, &Roots::File("/nonexistent/root.pem".into()));
        assert!(matches!(missing, Err(Error::Connect(message)) if message.contains("/nonexistent/root.pem")));
        let empty = std::env::temp_dir().join(format!("rustonrails-empty-{}.pem", std::process::id()));
        std::fs::write(&empty, "").unwrap();
        assert!(matches!(connector(Mode::VerifyCa, &Roots::File(empty.clone())), Err(Error::Connect(_))));
        // Nothing is verified under prefer or disable, so the file isn't read.
        assert!(connector(Mode::Prefer, &Roots::File(empty.clone())).is_ok());
        assert!(connector(Mode::Disable, &Roots::File("/nonexistent/root.pem".into())).is_ok());
        std::fs::remove_file(&empty).ok();
    }
}

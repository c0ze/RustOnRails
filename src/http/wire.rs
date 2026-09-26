//! HTTP/1.1 framing: reading a request's head and body off a connection and
//! writing a response back. Every read has a limit, so a client can't make
//! the server allocate what it merely claims to send.

use std::io::{self, BufRead, Read, Write};

use super::{Response, reason};

/// The largest request head (request line and headers) accepted.
pub const MAX_HEAD_BYTES: usize = 16 * 1024;

/// The longest chunk-size line in a chunked body, extensions included.
const MAX_CHUNK_LINE_BYTES: usize = 1024;

pub struct Head {
    pub method: String,
    pub target: String,
    /// 1 for `HTTP/1.1`, 0 for `HTTP/1.0`.
    pub minor_version: u8,
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// The first value of a header, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Comma-separated tokens across every copy of a header, lowercased.
    fn tokens(&self, name: &str) -> Vec<String> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.split(','))
            .map(|t| t.trim().to_ascii_lowercase())
            .collect()
    }

    /// HTTP/1.1 keeps the connection unless the client says `close`;
    /// HTTP/1.0 closes it unless the client says `keep-alive`.
    pub fn keep_alive(&self) -> bool {
        let connection = self.tokens("Connection");
        if self.minor_version == 0 {
            connection.iter().any(|t| t == "keep-alive")
        } else {
            !connection.iter().any(|t| t == "close")
        }
    }
}

/// Why a request couldn't be read.
#[derive(Debug, PartialEq)]
pub enum WireError {
    /// The client closed the connection, stalled past the timeout or broke
    /// it mid-request. There's nobody to answer.
    Gone,
    /// The request can't be served: answer with this status and close.
    Refuse(u16),
}

impl From<io::Error> for WireError {
    fn from(_: io::Error) -> Self {
        WireError::Gone
    }
}

/// Reads the request line and headers.
pub fn read_head(reader: &mut impl BufRead) -> Result<Head, WireError> {
    let mut budget = MAX_HEAD_BYTES;
    // Empty lines before the request line are ignored (RFC 9112 2.2).
    let request_line = loop {
        match read_line(reader, &mut budget, 431)? {
            None => return Err(WireError::Gone),
            Some(line) if line.is_empty() => continue,
            Some(line) => break line,
        }
    };
    let parts: Vec<&str> = request_line.split(' ').collect();
    let &[method, target, version] = parts.as_slice() else { return Err(WireError::Refuse(400)) };
    if method.is_empty() || !method.bytes().all(is_token) || !target.starts_with('/') || target.bytes().any(|b| b.is_ascii_control()) {
        return Err(WireError::Refuse(400));
    }
    let minor_version = match version {
        "HTTP/1.1" => 1,
        "HTTP/1.0" => 0,
        _ if version.starts_with("HTTP/") => return Err(WireError::Refuse(505)),
        _ => return Err(WireError::Refuse(400)),
    };
    let mut headers = Vec::new();
    loop {
        let Some(line) = read_line(reader, &mut budget, 431)? else { return Err(WireError::Gone) };
        if line.is_empty() {
            break;
        }
        // A name is a token: no whitespace before the colon, and no
        // obsolete line folding (RFC 9112 5.1, 5.2).
        let Some((name, value)) = line.split_once(':') else { return Err(WireError::Refuse(400)) };
        // A bare CR or other control byte in a value is how one header
        // hides another from a proxy that splits lines differently.
        if name.is_empty() || !name.bytes().all(is_token) || value.bytes().any(|b| b.is_ascii_control() && b != b'\t') {
            return Err(WireError::Refuse(400));
        }
        headers.push((name.to_string(), value.trim_matches([' ', '\t']).to_string()));
    }
    Ok(Head { method: method.to_string(), target: target.to_string(), minor_version, headers })
}

/// Reads the whole body, refusing one bigger than `max` before reading it.
/// A client that sent `Expect: 100-continue` is told to go ahead through
/// `interim` once the body is known to be acceptable.
pub fn read_body(head: &Head, reader: &mut impl BufRead, interim: &mut impl Write, max: usize) -> Result<Vec<u8>, WireError> {
    let lengths = head.tokens("Content-Length");
    let codings = head.tokens("Transfer-Encoding");
    // Both framings at once is how requests get smuggled (RFC 9112 6.3),
    // and HTTP/1.0 has no Transfer-Encoding to frame a body with (6.1).
    if !codings.is_empty() && (!lengths.is_empty() || head.minor_version == 0) {
        return Err(WireError::Refuse(400));
    }
    let chunked = match codings.as_slice() {
        [] => false,
        [only] if only == "chunked" => true,
        _ => return Err(WireError::Refuse(501)),
    };
    let length = content_length(&lengths)?;
    if length > max as u64 {
        return Err(WireError::Refuse(413));
    }
    let expects_continue = head.header("Expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue"));
    if (chunked || length > 0) && head.minor_version == 1 && expects_continue {
        interim.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        interim.flush()?;
    }
    if chunked {
        return read_chunked(reader, max);
    }
    let mut body = Vec::new();
    reader.by_ref().take(length).read_to_end(&mut body)?;
    // Fewer bytes than declared means the client went away mid-upload.
    if body.len() as u64 != length {
        return Err(WireError::Gone);
    }
    Ok(body)
}

/// Writes `response` in one piece. A HEAD request gets the headers a GET
/// would, without the body; 204 and 304 carry neither body nor length.
pub fn write_response(writer: &mut impl Write, response: &Response, head_only: bool, close: bool) -> io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\n", response.status, reason(response.status)).into_bytes();
    if let Some(content_type) = response.content_type {
        out.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
    }
    let bodyless = matches!(response.status, 100..=199 | 204 | 304);
    if !bodyless {
        out.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    }
    if close {
        out.extend_from_slice(b"Connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
    if !bodyless && !head_only {
        out.extend_from_slice(&response.body);
    }
    writer.write_all(&out)?;
    writer.flush()
}

/// One line without its line ending, charged against `budget`. `None` is a
/// clean end of input; a line longer than the budget is refused with
/// `too_long`.
fn read_line(reader: &mut impl BufRead, budget: &mut usize, too_long: u16) -> Result<Option<String>, WireError> {
    let mut line = Vec::new();
    let read = reader.by_ref().take(*budget as u64 + 1).read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if read > *budget {
        return Err(WireError::Refuse(too_long));
    }
    *budget -= read;
    if line.pop() != Some(b'\n') {
        return Err(WireError::Gone);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|_| WireError::Refuse(400))
}

/// Every Content-Length value must be the same run of digits.
fn content_length(values: &[String]) -> Result<u64, WireError> {
    let Some(first) = values.first() else { return Ok(0) };
    if values.iter().any(|v| v != first) || first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit()) {
        return Err(WireError::Refuse(400));
    }
    // More digits than a u64 holds is still just too large.
    Ok(first.parse().unwrap_or(u64::MAX))
}

/// A chunked body (RFC 9112 7.1): size lines with optional extensions, the
/// data, and a trailer section that is read and ignored.
fn read_chunked(reader: &mut impl BufRead, max: usize) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::new();
    loop {
        let mut budget = MAX_CHUNK_LINE_BYTES;
        let Some(line) = read_line(reader, &mut budget, 400)? else { return Err(WireError::Gone) };
        let size = line.split(';').next().unwrap_or("").trim_end_matches([' ', '\t']);
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(WireError::Refuse(400));
        }
        let size = usize::from_str_radix(size, 16).unwrap_or(usize::MAX);
        if size == 0 {
            break;
        }
        if size > max - body.len() {
            return Err(WireError::Refuse(413));
        }
        let before = body.len();
        reader.by_ref().take(size as u64).read_to_end(&mut body)?;
        if body.len() - before != size {
            return Err(WireError::Gone);
        }
        let mut budget = 2;
        match read_line(reader, &mut budget, 400)? {
            Some(end) if end.is_empty() => {}
            Some(_) => return Err(WireError::Refuse(400)),
            None => return Err(WireError::Gone),
        }
    }
    let mut budget = MAX_HEAD_BYTES;
    loop {
        match read_line(reader, &mut budget, 431)? {
            Some(line) if line.is_empty() => return Ok(body),
            Some(_) => continue,
            None => return Err(WireError::Gone),
        }
    }
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> Result<Head, WireError> {
        read_head(&mut raw.as_bytes())
    }

    fn body(raw: &str) -> Result<Vec<u8>, WireError> {
        let mut reader = raw.as_bytes();
        let head = read_head(&mut reader)?;
        read_body(&head, &mut reader, &mut Vec::new(), 64)
    }

    #[test]
    fn test_request_line_and_headers() {
        let head = head("\r\nGET /a?b=1 HTTP/1.0\r\nHost: x\r\nConnection: Keep-Alive\r\n\r\n").unwrap();
        assert_eq!(("GET", "/a?b=1", 0), (head.method.as_str(), head.target.as_str(), head.minor_version));
        assert_eq!(Some("x"), head.header("host"));
        assert!(head.keep_alive());
    }

    #[test]
    fn test_malformed_heads_are_refused() {
        assert_eq!(Some(WireError::Refuse(505)), head("GET / HTTP/2.0\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET  / HTTP/1.1\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET http://x/ HTTP/1.1\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET / HTTP/1.1\r\nHost : x\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET / HTTP/1.1\r\nA: b\r\n folded\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET / HTTP/1.1\r\nX: a\rContent-Length: 10\r\n\r\n").err());
        assert_eq!(Some(WireError::Refuse(400)), head("GET /a\x1b[2K HTTP/1.1\r\n\r\n").err());
        assert!(head("GET / HTTP/1.1\r\nX: a\tb\r\n\r\n").is_ok());
        let big = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD_BYTES));
        assert_eq!(Some(WireError::Refuse(431)), head(&big).err());
        assert_eq!(Some(WireError::Gone), head("GET / HTTP/1.1\r\nHost: x\r\n").err());
    }

    #[test]
    fn test_content_length_rules() {
        let post = |headers: &str, rest: &str| body(&format!("POST / HTTP/1.1\r\n{headers}\r\n{rest}"));
        assert_eq!(Ok(b"abc".to_vec()), post("Content-Length: 3\r\n", "abcdef"));
        assert_eq!(Ok(b"abc".to_vec()), post("Content-Length: 3\r\nContent-Length: 3\r\n", "abc"));
        assert_eq!(Err(WireError::Refuse(400)), post("Content-Length: 3\r\nContent-Length: 4\r\n", "abcd"));
        assert_eq!(Err(WireError::Refuse(400)), post("Content-Length: -3\r\n", "abc"));
        assert_eq!(Err(WireError::Refuse(413)), post("Content-Length: 99999999999999999999999\r\n", ""));
        assert_eq!(Err(WireError::Gone), post("Content-Length: 5\r\n", "abc"));
        assert_eq!(Err(WireError::Refuse(501)), post("Transfer-Encoding: gzip, chunked\r\n", "0\r\n\r\n"));
        let old = body("POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
        assert_eq!(Err(WireError::Refuse(400)), old);
    }

    #[test]
    fn test_chunked_rules() {
        let post = |rest: &str| body(&format!("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n{rest}"));
        assert_eq!(Ok(b"hello".to_vec()), post("2\r\nhe\r\n3;x=y\r\nllo\r\n0\r\nT: 1\r\n\r\n"));
        assert_eq!(Err(WireError::Refuse(400)), post("+2\r\nhe\r\n0\r\n\r\n"));
        assert_eq!(Err(WireError::Refuse(400)), post("2\r\nhexx\r\n0\r\n\r\n"));
        assert_eq!(Err(WireError::Refuse(413)), post("41\r\n"));
        assert_eq!(Err(WireError::Gone), post("2\r\nhe\r\n"));
    }

    #[test]
    fn test_head_responses_keep_the_length_and_drop_the_body() {
        let response = Response::json(200, serde_json::json!({"ok": true}));
        let mut out = Vec::new();
        write_response(&mut out, &response, true, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Content-Length: 11\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\n"), "{text}");
    }
}

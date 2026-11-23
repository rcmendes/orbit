//! Minimal bounded HTTP/1.x request parser and response writer.
//!
//! The parser is a trust boundary: every size is capped before data is read.

use std::io::{Read, Write};

pub const MAX_HEAD: usize = 8 * 1024;
pub const MAX_BODY: usize = 16 * 1024;
pub const MAX_HEADERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Maps to 400.
    BadRequest(&'static str),
    /// Maps to 413.
    TooLarge,
    /// Maps to 501.
    Unsupported,
}

impl ParseError {
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::TooLarge => 413,
            Self::Unsupported => 501,
        }
    }
}

fn read_head(stream: &mut impl Read) -> Result<Vec<u8>, ParseError> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(ParseError::TooLarge);
        }
        match stream.read(&mut byte) {
            Ok(0) => return Err(ParseError::BadRequest("truncated request")),
            Ok(_) => head.push(byte[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ParseError::BadRequest("read failed")),
        }
    }
    Ok(head)
}

pub fn read_request(stream: &mut impl Read) -> Result<Request, ParseError> {
    let head = read_head(stream)?;
    let head = std::str::from_utf8(&head[..head.len() - 4])
        .map_err(|_| ParseError::BadRequest("head is not utf-8"))?;
    let mut lines = head.split("\r\n");

    let mut parts = lines.next().unwrap_or("").split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && !t.is_empty() => (m, t, v),
        _ => return Err(ParseError::BadRequest("malformed request line")),
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ParseError::BadRequest("unsupported http version"));
    }
    let path = target.split(['?', '#']).next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    for line in lines {
        if headers.len() >= MAX_HEADERS {
            return Err(ParseError::TooLarge);
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(ParseError::BadRequest("malformed header"))?;
        if name.is_empty() || name.contains(|c: char| c.is_whitespace()) {
            return Err(ParseError::BadRequest("malformed header name"));
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ParseError::Unsupported);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ParseError::BadRequest("duplicate content-length"));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseError::BadRequest("invalid content-length"));
            }
            // Digits only: a parse failure here means overflow, which is too large.
            content_length = Some(value.parse().map_err(|_| ParseError::TooLarge)?);
        }
        headers.push((name.to_string(), value.to_string()));
    }

    let len = content_length.unwrap_or(0);
    if len > MAX_BODY {
        return Err(ParseError::TooLarge);
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .map_err(|_| ParseError::BadRequest("truncated body"))?;

    Ok(Request {
        method: method.to_string(),
        path,
        headers,
        body,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub extra: Vec<(&'static str, String)>,
}

impl Response {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
            extra: Vec::new(),
        }
    }

    pub fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: text.as_bytes().to_vec(),
            extra: Vec::new(),
        }
    }

    pub fn asset(content_type: &'static str, body: &'static str) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.as_bytes().to_vec(),
            extra: Vec::new(),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

pub fn write_response(stream: &mut impl Write, r: &Response) -> std::io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Content-Security-Policy: default-src 'self'\r\nReferrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.extra {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<Request, ParseError> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn parses_get_with_headers() {
        let r = parse(
            "GET /api/snapshot?x=1 HTTP/1.1\r\nHost: 127.0.0.1:9\r\nX-Orbit-Token: t\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/api/snapshot");
        assert_eq!(r.header("x-orbit-token"), Some("t"));
    }

    #[test]
    fn reads_body_by_content_length() {
        let r = parse("POST /api/move HTTP/1.1\r\nContent-Length: 7\r\n\r\n{\"a\":1}").unwrap();
        assert_eq!(r.body, b"{\"a\":1}");
    }

    #[test]
    fn rejects_transfer_encoding() {
        let e = parse("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap_err();
        assert_eq!(e, ParseError::Unsupported);
        assert_eq!(e.status(), 501);
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let e = parse("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx")
            .unwrap_err();
        assert!(matches!(e, ParseError::BadRequest(_)));
    }

    #[test]
    fn rejects_head_over_limit() {
        let raw = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(9000));
        assert_eq!(parse(&raw).unwrap_err(), ParseError::TooLarge);
    }

    #[test]
    fn rejects_body_over_limit_without_reading_it() {
        // No body bytes follow: a read attempt would yield BadRequest, not TooLarge.
        let e = parse("POST / HTTP/1.1\r\nContent-Length: 16385\r\n\r\n").unwrap_err();
        assert_eq!(e, ParseError::TooLarge);
        assert_eq!(e.status(), 413);
    }

    #[test]
    fn rejects_too_many_headers() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..65 {
            raw.push_str(&format!("H{i}: v\r\n"));
        }
        raw.push_str("\r\n");
        assert_eq!(parse(&raw).unwrap_err(), ParseError::TooLarge);
    }

    #[test]
    fn rejects_missing_http_version() {
        let e = parse("GET /\r\n\r\n").unwrap_err();
        assert!(matches!(e, ParseError::BadRequest(_)));
        assert_eq!(e.status(), 400);
    }

    #[test]
    fn rejects_truncated_body_and_bad_length() {
        assert!(matches!(
            parse("POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nab").unwrap_err(),
            ParseError::BadRequest(_)
        ));
        assert!(matches!(
            parse("POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n").unwrap_err(),
            ParseError::BadRequest(_)
        ));
    }

    #[test]
    fn response_carries_security_headers() {
        let mut out = Vec::new();
        write_response(&mut out, &Response::text(200, "hello")).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 200 OK\r\n"));
        for h in [
            "Content-Security-Policy: default-src 'self'",
            "Referrer-Policy: no-referrer",
            "X-Content-Type-Options: nosniff",
            "Connection: close",
            "Content-Length: 5",
        ] {
            assert!(s.contains(h), "missing {h}: {s}");
        }
        assert!(s.ends_with("\r\n\r\nhello"));
    }
}

//! Local-only gate for the Board view: Host/Origin allow-list plus a bearer token.

use super::http::Request;
use rand::RngCore;

/// Why a request was refused. `BadToken` maps to 401, the rest to 403.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    WrongHost,
    WrongOrigin,
    MissingOrigin,
    BadToken,
}

pub struct Guard {
    pub token: String,
    pub port: u16,
}

/// 32 random bytes as 64 lowercase hex chars.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Guard {
    pub fn check_host(&self, r: &Request) -> Result<(), Denied> {
        let p = self.port;
        match r.header("Host") {
            Some(h) if h == format!("127.0.0.1:{p}") || h == format!("localhost:{p}") => Ok(()),
            _ => Err(Denied::WrongHost),
        }
    }

    pub fn check_api(&self, r: &Request) -> Result<(), Denied> {
        self.check_host(r)?;
        let p = self.port;
        match r.header("Origin") {
            Some(o)
                if o == format!("http://127.0.0.1:{p}") || o == format!("http://localhost:{p}") => {
            }
            Some(_) => return Err(Denied::WrongOrigin),
            None if r.method == "POST" => return Err(Denied::MissingOrigin),
            None => {}
        }
        match r.header("X-Orbit-Token") {
            Some(t) if ct_eq(t.as_bytes(), self.token.as_bytes()) => Ok(()),
            _ => Err(Denied::BadToken),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        Guard {
            token: "a".repeat(64),
            port: 4747,
        }
    }

    fn req(method: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: method.into(),
            path: "/api/board".into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: vec![],
        }
    }

    fn tok() -> (&'static str, String) {
        ("X-Orbit-Token", "a".repeat(64))
    }

    #[test]
    fn token_is_64_hex_chars_and_random() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        for t in [a, b] {
            assert_eq!(t.len(), 64);
            assert!(t.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')));
        }
    }

    #[test]
    fn accepts_localhost_and_loopback_hosts() {
        for h in ["127.0.0.1:4747", "localhost:4747"] {
            assert_eq!(guard().check_host(&req("GET", &[("Host", h)])), Ok(()));
        }
    }

    #[test]
    fn rejects_rebinding_host() {
        let r = req("GET", &[("Host", "evil.example:4747")]);
        assert_eq!(guard().check_host(&r), Err(Denied::WrongHost));
        assert_eq!(guard().check_host(&req("GET", &[])), Err(Denied::WrongHost));
    }

    #[test]
    fn rejects_foreign_origin_even_with_token() {
        let t = tok();
        let r = req(
            "POST",
            &[
                ("Host", "127.0.0.1:4747"),
                ("Origin", "http://evil.example"),
                (t.0, &t.1),
            ],
        );
        assert_eq!(guard().check_api(&r), Err(Denied::WrongOrigin));
    }

    #[test]
    fn post_requires_origin() {
        let t = tok();
        let r = req("POST", &[("Host", "localhost:4747"), (t.0, &t.1)]);
        assert_eq!(guard().check_api(&r), Err(Denied::MissingOrigin));
    }

    #[test]
    fn get_without_origin_is_allowed() {
        let t = tok();
        let r = req("GET", &[("Host", "localhost:4747"), (t.0, &t.1)]);
        assert_eq!(guard().check_api(&r), Ok(()));
    }

    #[test]
    fn post_with_local_origin_is_allowed() {
        let t = tok();
        let r = req(
            "POST",
            &[
                ("Host", "localhost:4747"),
                ("Origin", "http://localhost:4747"),
                (t.0, &t.1),
            ],
        );
        assert_eq!(guard().check_api(&r), Ok(()));
    }

    #[test]
    fn rejects_wrong_or_missing_token() {
        let wrong = req(
            "GET",
            &[("Host", "localhost:4747"), ("X-Orbit-Token", "nope")],
        );
        assert_eq!(guard().check_api(&wrong), Err(Denied::BadToken));
        let missing = req("GET", &[("Host", "localhost:4747")]);
        assert_eq!(guard().check_api(&missing), Err(Denied::BadToken));
    }
}

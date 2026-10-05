//! `http://host:port/path?query` → [`Url`], plus resolving a redirect's
//! `Location` against the current URL.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Path plus query, always starting with `/`: what goes on the request
    /// line.
    pub path: String,
}

impl Url {
    /// The `Host` header value: the port is left out when it's the default.
    pub fn authority(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Resolves a redirect target against this URL. This is the common
    /// subset of RFC 3986 §5.2: absolute (`http://other/x`),
    /// scheme-relative (`//other/x`), absolute-path (`/x`), and relative
    /// (`x`, taken relative to the current directory).
    pub fn join(&self, location: &str) -> Result<Url, String> {
        if split_scheme(location).is_some() {
            return location.parse();
        }
        if let Some(rest) = location.strip_prefix("//") {
            return format!("http://{rest}").parse();
        }
        let path = if location.starts_with('/') {
            location.to_string()
        } else {
            let base = self.path.split('?').next().unwrap_or("/");
            let dir = &base[..=base.rfind('/').unwrap_or(0)];
            format!("{dir}{location}")
        };
        // Going through `parse` re-runs the validation on the new path.
        format!("http://{}{path}", self.authority()).parse()
    }
}

/// `http://x` → `("http", "x")`. A scheme is letters, digits, `+-.`, so the
/// `://` inside `/go?to=http://x` doesn't count.
fn split_scheme(s: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = s.split_once("://")?;
    let valid = scheme
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
    valid.then_some((scheme, rest))
}

/// `FromStr` is what makes `"http://x/".parse::<Url>()` work.
impl FromStr for Url {
    type Err = String;

    fn from_str(s: &str) -> Result<Url, String> {
        // Spaces or control characters would corrupt the request line, and a
        // CR/LF could inject headers.
        if s.bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(format!("URL contains spaces or control characters: {s:?}"));
        }
        let rest = match split_scheme(s) {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => {
                return Err("https:// needs TLS, which fetchr doesn't do; use http://".into());
            }
            Some((scheme, _)) => return Err(format!("unsupported scheme {scheme:?}")),
            None => s, // like curl, `host/path` with no scheme means http
        };
        // The authority (host[:port]) ends at the first '/', '?' or '#'.
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        // The #fragment is for the browser; it is never sent to the server.
        let tail = tail.split('#').next().unwrap_or_default();
        let path = if tail.starts_with('/') {
            tail.to_string()
        } else {
            format!("/{tail}")
        };

        if authority.contains('@') {
            return Err("user:password@host URLs are not supported".into());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse().map_err(|_| format!("bad port {port:?}"))?,
            ),
            None => (authority, 80),
        };
        if host.is_empty() {
            return Err(format!("no host in {s:?}"));
        }
        Ok(Url {
            host: host.to_string(),
            port,
            path,
        })
    }
}

/// `Display` gives us `to_string()` and `{url}` in format strings.
impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}{}", self.authority(), self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(host: &str, port: u16, path: &str) -> Url {
        Url {
            host: host.into(),
            port,
            path: path.into(),
        }
    }

    #[test]
    fn parses_urls() {
        for (input, want) in [
            ("http://example.com", url("example.com", 80, "/")),
            ("http://example.com/", url("example.com", 80, "/")),
            (
                "HTTP://Example.com:8080/a/b?x=1#top",
                url("Example.com", 8080, "/a/b?x=1"),
            ),
            ("http://127.0.0.1:8180?q=1", url("127.0.0.1", 8180, "/?q=1")),
            ("127.0.0.1:8180/files/", url("127.0.0.1", 8180, "/files/")),
            ("localhost", url("localhost", 80, "/")),
            ("http://h/go?to=http://x", url("h", 80, "/go?to=http://x")),
        ] {
            assert_eq!(input.parse::<Url>(), Ok(want), "{input}");
        }
    }

    #[test]
    fn rejects_bad_urls() {
        for (input, needle) in [
            ("https://example.com/", "TLS"),
            ("ftp://example.com/", "unsupported scheme"),
            ("http://", "no host"),
            ("http://:8080/", "no host"),
            ("http://h:99999/", "bad port"),
            ("http://h:http/", "bad port"),
            ("http://user:pw@h/", "not supported"),
            ("http://h/a b", "spaces"),
            ("http://h/a\r\nX-Evil: 1", "control"),
        ] {
            let err = input.parse::<Url>().unwrap_err();
            assert!(err.contains(needle), "{input}: {err}");
        }
    }

    #[test]
    fn displays_and_omits_the_default_port() {
        assert_eq!(url("h", 80, "/x").to_string(), "http://h/x");
        assert_eq!(url("h", 8080, "/x").to_string(), "http://h:8080/x");
    }

    #[test]
    fn joins_redirect_locations() {
        let base = url("h", 8080, "/a/b?x=1");
        for (location, want) in [
            ("http://other/z", url("other", 80, "/z")),
            ("//other:81/z", url("other", 81, "/z")),
            ("/root?y=2", url("h", 8080, "/root?y=2")),
            ("c", url("h", 8080, "/a/c")),
            ("c/d", url("h", 8080, "/a/c/d")),
        ] {
            assert_eq!(base.join(location), Ok(want), "{location}");
        }
        assert!(base.join("https://secure/").is_err());
    }
}

//! From URL to file: percent-decoding, `..` handling, MIME types, and
//! directory listings.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::http::Response;

/// Turns a request target like `/docs/a%20b.txt?x=1` into a path under
/// `root`. It is purely lexical (no filesystem access), so it's easy to
/// test. Errors are statuses: 400 for a target we can't decode, 403 for one
/// that climbs out of the root.
pub fn resolve(root: &Path, target: &str) -> Result<PathBuf, u16> {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    // Only origin-form (RFC 9112 §3.2.1). `*` and `http://host/x` aren't
    // file paths.
    if !path.starts_with('/') {
        return Err(400);
    }
    // Decode first, then split. In the other order, `..%2F..%2Fetc` would
    // pass the `..` check as one harmless-looking segment.
    let decoded = percent_decode(path).ok_or(400u16)?;
    let mut out = root.to_path_buf();
    let mut depth = 0;
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." if depth == 0 => return Err(403),
            ".." => {
                out.pop();
                depth -= 1;
            }
            // Segments never contain '/', which matters: `push("/etc")`
            // would *replace* the whole path instead of appending to it.
            name => {
                out.push(name);
                depth += 1;
            }
        }
    }
    Ok(out)
}

/// `%41` becomes `A`. Returns None for a broken escape, for bytes that
/// aren't UTF-8, and for NUL (which no file name may contain).
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = char::from(*bytes.get(i + 1)?).to_digit(16)?;
            let lo = char::from(*bytes.get(i + 2)?).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

/// The Content-Type for a file, chosen by its extension.
pub fn mime_type(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" | "rs" | "toml" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Reads a whole file into the response body.
pub fn serve_file(path: &Path) -> Response {
    match fs::read(path) {
        Ok(bytes) => Response::new(200, mime_type(path), bytes),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Response::plain(403),
        Err(_) => Response::plain(404),
    }
}

/// An HTML page listing `dir`. `url_path` is the (still encoded) URL of
/// the directory, ending in `/`.
pub fn serve_listing(dir: &Path, url_path: &str) -> Response {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Response::plain(403);
    };
    let mut entries: Vec<(String, bool)> = read_dir
        .filter_map(Result::ok)
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (e.file_name().to_string_lossy().into_owned(), is_dir)
        })
        .collect();
    entries.sort();
    let title = percent_decode(url_path).unwrap_or_else(|| url_path.to_string());
    Response::new(
        200,
        "text/html; charset=utf-8",
        listing_html(&title, &entries),
    )
}

/// Pure HTML generation, so the escaping is unit-testable. Names are
/// attacker-controlled (anyone who can create a file), so the text is
/// HTML-escaped and the link is percent-encoded.
pub fn listing_html(title: &str, entries: &[(String, bool)]) -> String {
    let title = html_escape(title);
    let mut html = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>Index of {title}</title>\n\
         <link rel=\"stylesheet\" href=\"/style.css\">\n<main>\n<h1>Index of {title}</h1>\n<ul>\n"
    );
    if title != "/" {
        html.push_str("<li><a href=\"../\">../</a></li>\n");
    }
    for (name, is_dir) in entries {
        let slash = if *is_dir { "/" } else { "" };
        let (href, text) = (href_encode(name), html_escape(name));
        html.push_str(&format!(
            "<li><a href=\"{href}{slash}\">{text}{slash}</a></li>\n"
        ));
    }
    html.push_str("</ul>\n</main>\n");
    html
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Percent-encodes every byte except the RFC 3986 "unreserved" ones.
fn href_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_targets_inside_the_root() {
        let root = Path::new("/srv");
        for (target, want) in [
            ("/", "/srv"),
            ("/style.css", "/srv/style.css"),
            ("/files/a%20b.txt", "/srv/files/a b.txt"),
            ("/files/notes.txt?download=1", "/srv/files/notes.txt"),
            ("//files/./x/../notes.txt", "/srv/files/notes.txt"),
            ("/%E2%9C%93.txt", "/srv/\u{2713}.txt"),
        ] {
            assert_eq!(resolve(root, target), Ok(PathBuf::from(want)), "{target}");
        }
    }

    #[test]
    fn refuses_targets_that_escape_or_dont_decode() {
        let root = Path::new("/srv");
        for (target, status) in [
            ("/../etc/passwd", 403),
            ("/a/../../etc/passwd", 403),
            ("/%2e%2e/etc/passwd", 403),
            ("/a/..%2F..%2Fetc", 403),
            ("/%zz", 400),
            ("/%4", 400),
            ("/a%00.txt", 400),
            ("/%ff", 400), // not UTF-8
            ("*", 400),
            ("http://example.com/", 400),
        ] {
            assert_eq!(resolve(root, target), Err(status), "{target}");
        }
    }

    #[test]
    fn picks_mime_types_by_extension() {
        assert_eq!(
            mime_type(Path::new("index.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(mime_type(Path::new("a/b.css")), "text/css; charset=utf-8");
        assert_eq!(mime_type(Path::new("logo.png")), "image/png");
        assert_eq!(mime_type(Path::new("Makefile")), "application/octet-stream");
    }

    #[test]
    fn listing_escapes_names_and_links() {
        let html = listing_html(
            "/files/",
            &[("<b>&x.txt".into(), false), ("sub".into(), true)],
        );
        assert!(html.contains("<a href=\"%3Cb%3E%26x.txt\">&lt;b&gt;&amp;x.txt</a>"));
        assert!(html.contains("<a href=\"sub/\">sub/</a>"));
        assert!(html.contains("<a href=\"../\">"));
        assert!(!listing_html("/", &[]).contains("../"));
    }
}

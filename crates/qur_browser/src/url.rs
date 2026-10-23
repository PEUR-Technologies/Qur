/// Turn what the user typed into a loadable URL.
///
/// - Anything with a scheme (`http://`, `file://`, `about:`, `data:` ...) is kept.
/// - Absolute or `~/` paths become `file://` URLs (percent-encoded).
/// - `localhost`, `*.localhost`, `127.x`, `0.0.0.0`, `[::1]`, or any `host:port` get `http://`.
/// - Everything else gets `https://`.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return "about:blank".into();
    }
    if s.contains("://") || ["about:", "data:", "blob:"].iter().any(|p| starts_with_ci(s, p)) {
        return s.into();
    }
    if s.starts_with('/') || s.starts_with("~/") || s == "~" {
        let path = match s.strip_prefix('~') {
            Some(rest) => format!("{}{rest}", std::env::var("HOME").unwrap_or_default()),
            None => s.to_string(),
        };
        return format!("file://{}", percent_encode_path(&path));
    }
    let authority = s.split(['/', '?', '#']).next().unwrap_or(s);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    let has_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let host_lc = host.to_ascii_lowercase();
    let local = host_lc == "localhost"
        || host_lc.ends_with(".localhost")
        || host_lc.starts_with("127.")
        || host_lc == "0.0.0.0"
        || host_lc == "::1";
    let scheme = if local || has_port { "http" } else { "https" };
    format!("{scheme}://{s}")
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize_url as n;

    #[test]
    fn local_hosts_get_http() {
        assert_eq!(n("localhost:3000"), "http://localhost:3000");
        assert_eq!(n("localhost"), "http://localhost");
        assert_eq!(n("127.0.0.1:8080/a?b#c"), "http://127.0.0.1:8080/a?b#c");
        assert_eq!(n("app.localhost/x"), "http://app.localhost/x");
        assert_eq!(n("0.0.0.0:5173"), "http://0.0.0.0:5173");
        assert_eq!(n("[::1]:4000"), "http://[::1]:4000");
        assert_eq!(n("myhost:8000"), "http://myhost:8000");
    }

    #[test]
    fn public_hosts_get_https() {
        assert_eq!(n("example.com"), "https://example.com");
        assert_eq!(n("  example.com/path?q=1 "), "https://example.com/path?q=1");
        assert_eq!(n("docs.rs/objc2"), "https://docs.rs/objc2");
    }

    #[test]
    fn schemes_are_kept() {
        assert_eq!(n("http://example.com"), "http://example.com");
        assert_eq!(n("https://localhost:3000"), "https://localhost:3000");
        assert_eq!(n("about:blank"), "about:blank");
        assert_eq!(n("file:///tmp/a.html"), "file:///tmp/a.html");
        assert_eq!(n(""), "about:blank");
    }

    #[test]
    fn paths_become_file_urls() {
        assert_eq!(n("/tmp/a b.html"), "file:///tmp/a%20b.html");
        assert_eq!(n("/tmp/#x?.html"), "file:///tmp/%23x%3F.html");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(n("~/x.html"), format!("file://{home}/x.html"));
    }
}

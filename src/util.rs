//! Мелкие вспомогательные функции: разбор URL, экранирование строк для JS и JSON, чтение списка файлов.

use std::fs;
use std::path::{Path, PathBuf};
use crate::config::*;

pub(crate) fn is_under(host: &str, base: &str) -> bool {
    host == base
        || (host.len() > base.len()
            && host.ends_with(base)
            && host.as_bytes()[host.len() - base.len() - 1] == b'.')
}

pub(crate) fn is_under_any(host: &str, list: &[&str]) -> bool {
    list.iter().any(|b| is_under(host, b))
}

pub(crate) fn valid_domain(d: &str) -> bool {
    let n = d.len();
    if n < 4 || n > 253 || !d.contains('.') || d.starts_with('.') || d.ends_with('.') || d.starts_with('-') || d.contains("..") {
        return false;
    }
    if !d.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') {
        return false;
    }
    !d.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

pub(crate) fn list_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect::<Vec<PathBuf>>())
        .unwrap_or_default();
    files.retain(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        !name.starts_with('_') && p.extension().and_then(|e| e.to_str()).map_or(false, |e| e.eq_ignore_ascii_case(ext))
    });
    files.sort();
    files
}

pub(crate) fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' | '\r' | '\t' => o.push(' '),
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub(crate) fn json_arr(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}

pub(crate) fn js_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\'' => o.push_str("\\'"),
            '\n' | '\r' => {}
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub(crate) fn make_secret() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = String::with_capacity(48);
    for i in 0..3u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split(|c| c == '/' || c == '\\' || c == '?' || c == '#').next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if let Some(v6) = auth.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        auth.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

pub(crate) fn perf_log(dir: &Path, line: &str) {
    use std::io::Write;
    let path = dir.join("perf.log");
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 300_000 {
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86400;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{:02}:{:02}:{:02} {}", ts / 3600, ts % 3600 / 60, ts % 60, line);
    }
}

pub(crate) fn is_http_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() > 8
        && s.len() <= MAX_URL_LEN
        && !s.bytes().any(|b| b <= b' ' || b == 0x7f)
        && !s.contains("|||")
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn clean_title(t: &str) -> String {
    t.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string()
}

pub(crate) fn json_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' || c == '\u{2028}' || c == '\u{2029}' => {
                o.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_extracted() {
        assert_eq!(host_of("https://User@Example.com:8080/path?q=1"), Some("example.com".to_string()));
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn http_url_check() {
        assert!(is_http_url("https://a.b"));
        assert!(!is_http_url("http://"));
        assert!(!is_http_url("ftp://example.com"));
        assert!(!is_http_url("https://a b.com"));
    }

    #[test]
    fn subdomain_check() {
        assert!(is_under("a.example.com", "example.com"));
        assert!(is_under("example.com", "example.com"));
        assert!(!is_under("badexample.com", "example.com"));
    }

    #[test]
    fn js_string_is_escaped() {
        assert_eq!(js_str("it's\n"), "it\\'s");
        assert_eq!(js_str("a\\b"), "a\\\\b");
    }
}
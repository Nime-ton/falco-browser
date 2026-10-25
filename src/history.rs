//! История и закладки (текстовые файлы в папке приложения).

use std::fs;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use crate::tabs::*;
use crate::util::*;

pub(crate) const HISTORY_SKIP_LOCAL: bool = true;

pub(crate) const HISTORY_MAX: usize = 3000;

pub(crate) const HISTORY_SHOW: usize = 400;

pub(crate) const BOOKMARKS_MAX: usize = 2000;

pub(crate) const FIELD_SEP: char = '\u{1f}';

pub(crate) struct Entry {
    pub(crate) ts: u64,
    pub(crate) url: String,
    pub(crate) title: String,
}

pub(crate) fn is_local_host(url: &str) -> bool {
    let h = match host_of(url) {
        Some(h) => h,
        None => return false,
    };
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private() || ip.is_link_local();
    }
    if let Ok(ip) = h.parse::<std::net::Ipv6Addr>() {
        return ip.is_loopback();
    }
    false
}

pub(crate) fn read_entries(path: &Path) -> Vec<Entry> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, FIELD_SEP);
            let ts = it.next()?.parse::<u64>().ok()?;
            let url = it.next()?;
            if !is_http_url(url) {
                return None;
            }
            let title = clean_title(it.next().unwrap_or(""));
            Some(Entry { ts, url: url.to_string(), title })
        })
        .collect()
}

pub(crate) fn write_entries(path: &Path, list: &[Entry]) {
    let mut s = String::new();
    for e in list {
        s.push_str(&format!("{}{}{}{}{}\n", e.ts, FIELD_SEP, e.url, FIELD_SEP, e.title));
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

pub(crate) fn append_entry(path: &Path, e: &Entry) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}{}{}{}{}", e.ts, FIELD_SEP, e.url, FIELD_SEP, e.title);
    }
}

pub(crate) fn history_view(path: &Path, limit: usize) -> Vec<Entry> {
    let mut all = read_entries(path);
    all.reverse();
    let mut seen: HashSet<String> = HashSet::new();
    all.retain(|e| seen.insert(e.url.clone()));
    all.truncate(limit);
    all
}

pub(crate) fn compact_history(path: &Path) {
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len < 600_000 {
        return;
    }
    let mut keep = history_view(path, HISTORY_MAX);
    keep.reverse();
    write_entries(path, &keep);
}

pub(crate) fn history_del(path: &Path, url: &str) {
    let mut list = read_entries(path);
    let before = list.len();
    list.retain(|e| e.url != url);
    if list.len() != before {
        write_entries(path, &list);
    }
}

pub(crate) struct HistState {
    pub(crate) pending: Option<Entry>,
    pub(crate) last: Option<(String, String)>,
    pub(crate) enabled: bool,
}

impl HistState {
    pub(crate) fn new(enabled: bool) -> Self {
        HistState { pending: None, last: None, enabled }
    }

    pub(crate) fn note(&mut self, path: &Path, url: &str, title: &str) {
        let same = self.pending.as_ref().map_or(false, |p| p.url == url);
        if same {
            if let Some(p) = self.pending.as_mut() {
                p.title = title.to_string();
            }
        } else {
            self.flush(path);
            self.pending = Some(Entry { ts: now_secs(), url: url.to_string(), title: title.to_string() });
        }
    }

    pub(crate) fn leave(&mut self, path: &Path, new_url: &str) {
        let other = self.pending.as_ref().map_or(false, |p| p.url != new_url);
        if other {
            self.flush(path);
        }
    }

    pub(crate) fn flush(&mut self, path: &Path) {
        if let Some(e) = self.pending.take() {
            if !self.enabled || e.title.is_empty() || !is_http_url(&e.url) {
                return;
            }
            if HISTORY_SKIP_LOCAL && is_local_host(&e.url) {
                return;
            }
            let key = (e.url.clone(), e.title.clone());
            if self.last.as_ref() == Some(&key) {
                return;
            }
            append_entry(path, &e);
            self.last = Some(key);
            if fs::metadata(path).map(|m| m.len()).unwrap_or(0) > 1_500_000 {
                compact_history(path);
            }
        }
    }
}

pub(crate) fn bm_add(path: &Path, url: &str, title: &str) -> String {
    let mut list = read_entries(path);
    if let Some(e) = list.iter().find(|e| e.url == url) {
        return e.title.clone();
    }
    if list.len() >= BOOKMARKS_MAX {
        list.remove(0);
    }
    let title = if title.is_empty() { tab_label(url) } else { title.to_string() };
    list.push(Entry { ts: now_secs(), url: url.to_string(), title: title.clone() });
    write_entries(path, &list);
    title
}

pub(crate) fn bm_rename(path: &Path, url: &str, title: &str) -> bool {
    let mut list = read_entries(path);
    let mut changed = false;
    for e in list.iter_mut() {
        if e.url == url {
            e.title = title.to_string();
            changed = true;
        }
    }
    if changed {
        write_entries(path, &list);
    }
    changed
}

pub(crate) fn bm_state_js(map: &HashMap<String, String>, url: &str) -> String {
    match map.get(url) {
        Some(t) => format!("if(window.__ulBM) window.__ulBM(true,'{}');", js_str(t)),
        None => "if(window.__ulBM) window.__ulBM(false);".to_string(),
    }
}

pub(crate) fn bm_del(path: &Path, url: &str) {
    history_del(path, url);
}

pub(crate) fn bm_view(path: &Path) -> Vec<Entry> {
    let mut list = read_entries(path);
    list.reverse();
    list
}

pub(crate) fn entries_json(list: &[Entry]) -> String {
    let mut o = String::from("[");
    for (i, e) in list.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!("{{\"u\":{},\"t\":{},\"d\":{}}}", json_quote(&e.url), json_quote(&e.title), e.ts));
    }
    o.push(']');
    o
}

pub(crate) fn panel_js(kind: &str, list: &[Entry]) -> String {
    format!("if(window.__ulPanel) window.__ulPanel('{}','{}');", kind, js_str(&entries_json(list)))
}
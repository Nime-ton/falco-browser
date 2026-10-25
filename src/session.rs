//! Положение окна и сессия вкладок.

use std::fs;
use std::path::Path;
use crate::config::*;
use crate::util::*;

pub(crate) struct WinState {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) max: bool,
}

pub(crate) fn load_win_state(path: &Path) -> Option<WinState> {
    let text = fs::read_to_string(path).ok()?;
    let mut it = text.split_whitespace();
    let x = it.next()?.parse::<i32>().ok()?;
    let y = it.next()?.parse::<i32>().ok()?;
    let w = it.next()?.parse::<u32>().ok()?;
    let h = it.next()?.parse::<u32>().ok()?;
    let max = it.next().unwrap_or("0") == "1";
    if w < 200 || h < 150 || x <= -10000 || y <= -10000 {
        return None;
    }
    Some(WinState { x, y, w, h, max })
}

pub(crate) fn save_win_state(path: &Path, st: &WinState) {
    let s = format!("{} {} {} {} {}\n", st.x, st.y, st.w, st.h, if st.max { 1 } else { 0 });
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Файл сессии: первая строка "<активная> <закреплённых>" (старый формат - только активная), дальше по URL в строке.
pub(crate) fn load_session(path: &Path) -> (Vec<String>, usize, usize) {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    let mut nums = head.split_whitespace().map(|x| x.parse::<usize>().ok());
    let active = nums.next().flatten().unwrap_or(0);
    let pinned = nums.next().flatten().unwrap_or(0);
    let tabs: Vec<String> = lines
        .map(|l| l.trim())
        .filter(|l| is_http_url(l))
        .take(MAX_TABS)
        .map(|l| l.to_string())
        .collect();
    let active = if tabs.is_empty() { 0 } else { active.min(tabs.len() - 1) };
    let pinned = pinned.min(tabs.len());
    (tabs, active, pinned)
}

pub(crate) fn save_session(path: &Path, last: &mut String, tabs: &[String], active: usize, pinned: usize) {
    let mut s = format!("{} {}", active, pinned.min(tabs.len()));
    for t in tabs {
        s.push('\n');
        s.push_str(t);
    }
    if s == *last {
        return;
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &s).is_ok() && fs::rename(&tmp, path).is_ok() {
        *last = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("falco-test-{}-{}.txt", name, std::process::id()))
    }

    #[test]
    fn session_roundtrip_keeps_pinned() {
        let path = tmp("roundtrip");
        let tabs = vec!["https://a.com/".to_string(), "https://b.com/".to_string(), "https://c.com/".to_string()];
        let mut last = String::new();
        save_session(&path, &mut last, &tabs, 2, 1);
        let (t, active, pinned) = load_session(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(t, tabs);
        assert_eq!((active, pinned), (2, 1));
    }

    #[test]
    fn old_session_format_still_loads() {
        let path = tmp("old");
        let _ = fs::write(&path, "1\nhttps://a.com/\nhttps://b.com/\n");
        let (t, active, pinned) = load_session(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(t.len(), 2);
        assert_eq!((active, pinned), (1, 0));
    }

    #[test]
    fn pinned_is_clamped_to_tab_count() {
        let path = tmp("clamp");
        let _ = fs::write(&path, "0 9\nhttps://a.com/\n");
        let (_, _, pinned) = load_session(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(pinned, 1);
    }
}
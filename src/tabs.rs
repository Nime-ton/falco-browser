//! Вкладки: ярлыки, проверка навигации, JS-команды и фильтр устаревших сообщений от страниц.

use std::collections::HashMap;
use crate::util::*;

pub(crate) fn nav_script(url: &str, save_first: bool) -> String {
    let u = js_str(url);
    if save_first {
        format!(
            "if (window.__ulNavSmooth) window.__ulNavSmooth('{}'); else {{ if (window.__ulSaveForSwitch) window.__ulSaveForSwitch(); window.location.href = '{}'; }}",
            u, u
        )
    } else {
        format!(
            "if (window.__ulNavDirect) window.__ulNavDirect('{}'); else window.location.href = '{}';",
            u, u
        )
    }
}

/// Сообщения SYNC_URL и TITLE приходят с id документа: "SYNC_URL|<doc>|<url>", "TITLE|<doc>|<title>".
/// Новый документ объявляет себя сообщением "LOAD_URL|<doc>|<url>". Rust сбрасывает `cur_doc`
/// при каждой своей навигации (переключение, закрытие вкладки и т.д.), поэтому запоздавшие
/// сообщения старой страницы отбрасываются и не портят URL/заголовок уже новой вкладки.
/// Остальные сообщения возвращаются как есть; SYNC_URL/TITLE - в прежнем формате без id.
pub(crate) fn filter_doc_msg(msg: &str, cur_doc: &mut Option<String>) -> Option<String> {
    let valid = |d: &str| !d.is_empty() && d.len() <= 40 && d.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Some(rest) = msg.strip_prefix("LOAD_URL|") {
        let (doc, url) = rest.split_once('|')?;
        if !valid(doc) {
            return None;
        }
        *cur_doc = Some(doc.to_string());
        return Some(format!("SYNC_URL|{}", url));
    }
    for prefix in ["SYNC_URL|", "TITLE|"] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            let (doc, payload) = rest.split_once('|')?;
            if !valid(doc) || cur_doc.as_deref() != Some(doc) {
                return None;
            }
            return Some(format!("{}{}", prefix, payload));
        }
    }
    Some(msg.to_string())
}

pub(crate) fn tab_label(url: &str) -> String {
    match host_of(url) {
        Some(h) => h.strip_prefix("www.").map(|x| x.to_string()).unwrap_or(h),
        None => "Новая вкладка".to_string(),
    }
}

/// Это пустая стартовая страница (то, что открывается вместо закрытой последней вкладки)?
pub(crate) fn is_start_page(url: &str, start_url: &str) -> bool {
    url.trim_end_matches('/') == start_url.trim_end_matches('/')
}

/// Строка для JS: "<сколько закреплено>\u{1e}<ярлык>|||<ярлык>|||...".
/// Закреплённые вкладки всегда идут первыми.
pub(crate) fn tabs_labels(tabs: &[String], titles: &HashMap<String, String>, pinned: usize) -> String {
    let labels = tabs
        .iter()
        .map(|u| match titles.get(u) {
            Some(t) if !t.is_empty() => t.clone(),
            _ => tab_label(u),
        })
        .collect::<Vec<_>>()
        .join("|||");
    format!("{}\u{1e}{}", pinned.min(tabs.len()), labels)
}

// ---------- Операции над списком вкладок (меню правой кнопки) ----------
// `pinned` - число закреплённых вкладок; они всегда стоят в начале списка.

/// Закрепляет вкладку `idx`: переносит её в конец блока закреплённых.
pub(crate) fn pin_tab(tabs: &mut Vec<String>, current: &mut usize, pinned: &mut usize, idx: usize) -> bool {
    if idx >= tabs.len() || idx < *pinned {
        return false;
    }
    let at = *pinned;
    let t = tabs.remove(idx);
    tabs.insert(at, t);
    if *current == idx {
        *current = at;
    } else if *current >= at && *current < idx {
        *current += 1;
    }
    *pinned += 1;
    true
}

/// Открепляет вкладку `idx`: она становится первой среди обычных.
pub(crate) fn unpin_tab(tabs: &mut Vec<String>, current: &mut usize, pinned: &mut usize, idx: usize) -> bool {
    if idx >= tabs.len() || idx >= *pinned {
        return false;
    }
    *pinned -= 1;
    let at = *pinned;
    let t = tabs.remove(idx);
    tabs.insert(at, t);
    if *current == idx {
        *current = at;
    } else if *current > idx && *current <= at {
        *current -= 1;
    }
    true
}

/// Вставляет `url` справа от вкладки `idx` (но не внутрь блока закреплённых) и делает её текущей.
pub(crate) fn insert_right(tabs: &mut Vec<String>, current: &mut usize, pinned: usize, idx: usize, url: String) -> usize {
    let at = (idx + 1).max(pinned).min(tabs.len());
    tabs.insert(at, url);
    *current = at;
    at
}

/// "Закрыть другие вкладки": остаются закреплённые и `idx`.
/// Возвращает закрытые вкладки (url, старый индекс) и флаг "текущую вкладку закрыли - нужно загрузить другую страницу".
/// Если текущая вкладка уцелела, она остаётся текущей, иначе текущей становится `idx`.
pub(crate) fn close_others(tabs: &mut Vec<String>, current: &mut usize, pinned: usize, idx: usize) -> (Vec<(String, usize)>, bool) {
    let old = std::mem::take(tabs);
    let mut removed = Vec::new();
    let mut new_current = None;
    let mut new_idx = 0;
    for (i, url) in old.into_iter().enumerate() {
        if i < pinned || i == idx {
            if i == *current {
                new_current = Some(tabs.len());
            }
            if i == idx {
                new_idx = tabs.len();
            }
            tabs.push(url);
        } else {
            removed.push((url, i));
        }
    }
    let nav = new_current.is_none();
    *current = new_current.unwrap_or(new_idx);
    (removed, nav)
}

/// "Закрыть вкладки справа": закрываются все обычные вкладки правее `idx`.
/// Если текущая вкладка закрыта, текущей становится `idx` (и флаг `true`).
pub(crate) fn close_right(tabs: &mut Vec<String>, current: &mut usize, pinned: usize, idx: usize) -> (Vec<(String, usize)>, bool) {
    let old = std::mem::take(tabs);
    let mut removed = Vec::new();
    for (i, url) in old.into_iter().enumerate() {
        if i > idx && i >= pinned {
            removed.push((url, i));
        } else {
            tabs.push(url);
        }
    }
    let nav = *current > idx && *current >= pinned;
    if nav {
        *current = idx;
    }
    (removed, nav)
}

pub(crate) fn nav_allowed(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "about" | "blob" | "data" | "chrome-error"
        ),
        None => false,
    }
}

pub(crate) fn spawn_ok(last: &mut Option<std::time::Instant>) -> bool {
    let now = std::time::Instant::now();
    if let Some(t) = last {
        if now.duration_since(*t).as_millis() < 1500 {
            return false;
        }
    }
    *last = Some(now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_messages_are_dropped() {
        let mut cur: Option<String> = None;
        assert_eq!(
            filter_doc_msg("LOAD_URL|aaa|https://a.com/", &mut cur),
            Some("SYNC_URL|https://a.com/".to_string())
        );
        assert_eq!(
            filter_doc_msg("TITLE|aaa|Hello|World", &mut cur),
            Some("TITLE|Hello|World".to_string())
        );
        cur = None;
        assert_eq!(filter_doc_msg("SYNC_URL|aaa|https://a.com/#x", &mut cur), None);
        assert_eq!(filter_doc_msg("TITLE|aaa|Old", &mut cur), None);
        assert_eq!(
            filter_doc_msg("LOAD_URL|bbb|https://b.com/", &mut cur),
            Some("SYNC_URL|https://b.com/".to_string())
        );
        assert_eq!(filter_doc_msg("SYNC_URL|aaa|https://a.com/", &mut cur), None);
        assert_eq!(
            filter_doc_msg("SYNC_URL|bbb|https://b.com/x", &mut cur),
            Some("SYNC_URL|https://b.com/x".to_string())
        );
    }

    #[test]
    fn other_messages_pass_and_garbage_is_dropped() {
        let mut cur: Option<String> = None;
        assert_eq!(filter_doc_msg("GET_TABS", &mut cur), Some("GET_TABS".to_string()));
        assert_eq!(filter_doc_msg("TITLE|bad id|x", &mut cur), None);
        assert_eq!(filter_doc_msg("LOAD_URL|nopipe", &mut cur), None);
    }
}

#[cfg(test)]
mod tab_ops_tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn labels_carry_pinned_count() {
        let titles = HashMap::new();
        let tabs = v(&["https://www.example.com/", "https://b.org/x"]);
        assert_eq!(tabs_labels(&tabs, &titles, 1), "1\u{1e}example.com|||b.org");
        assert_eq!(tabs_labels(&tabs, &titles, 9), "2\u{1e}example.com|||b.org");
    }

    #[test]
    fn pin_moves_tab_to_end_of_pinned_block() {
        let mut tabs = v(&["a", "b", "c", "d"]);
        let (mut cur, mut pinned) = (3, 1);
        assert!(pin_tab(&mut tabs, &mut cur, &mut pinned, 3));
        assert_eq!(tabs, v(&["a", "d", "b", "c"]));
        assert_eq!((cur, pinned), (1, 2));
    }

    #[test]
    fn pin_keeps_current_tab_selected() {
        let mut tabs = v(&["a", "b", "c", "d"]);
        let (mut cur, mut pinned) = (2, 0);
        assert!(pin_tab(&mut tabs, &mut cur, &mut pinned, 3));
        assert_eq!(tabs, v(&["d", "a", "b", "c"]));
        assert_eq!((cur, pinned), (3, 1));
        assert_eq!(tabs[cur], "c");
    }

    #[test]
    fn pin_rejects_already_pinned_or_bad_index() {
        let mut tabs = v(&["a", "b"]);
        let (mut cur, mut pinned) = (0, 1);
        assert!(!pin_tab(&mut tabs, &mut cur, &mut pinned, 0));
        assert!(!pin_tab(&mut tabs, &mut cur, &mut pinned, 5));
    }

    #[test]
    fn unpin_puts_tab_right_after_pinned_block() {
        let mut tabs = v(&["a", "b", "c", "d"]);
        let (mut cur, mut pinned) = (0, 3);
        assert!(unpin_tab(&mut tabs, &mut cur, &mut pinned, 0));
        assert_eq!(tabs, v(&["b", "c", "a", "d"]));
        assert_eq!((cur, pinned), (2, 2));
    }

    #[test]
    fn unpin_keeps_current_tab_selected() {
        let mut tabs = v(&["a", "b", "c", "d"]);
        let (mut cur, mut pinned) = (1, 3);
        assert!(unpin_tab(&mut tabs, &mut cur, &mut pinned, 0));
        assert_eq!(tabs[cur], "b");
    }

    #[test]
    fn insert_right_skips_pinned_block() {
        let mut tabs = v(&["a", "b", "c"]);
        let mut cur = 0;
        assert_eq!(insert_right(&mut tabs, &mut cur, 2, 0, "X".to_string()), 2);
        assert_eq!(tabs, v(&["a", "b", "X", "c"]));
        assert_eq!(cur, 2);
        let mut tabs = v(&["a", "b", "c"]);
        assert_eq!(insert_right(&mut tabs, &mut cur, 2, 2, "X".to_string()), 3);
        assert_eq!(tabs, v(&["a", "b", "c", "X"]));
    }

    #[test]
    fn close_others_keeps_pinned_and_target() {
        let mut tabs = v(&["p1", "p2", "a", "b", "c"]);
        let mut cur = 0;
        let (removed, nav) = close_others(&mut tabs, &mut cur, 2, 3);
        assert_eq!(tabs, v(&["p1", "p2", "b"]));
        assert_eq!(removed, vec![("a".to_string(), 2), ("c".to_string(), 4)]);
        assert!(!nav);
        assert_eq!(cur, 0);
    }

    #[test]
    fn close_others_switches_when_current_is_closed() {
        let mut tabs = v(&["p1", "p2", "a", "b", "c"]);
        let mut cur = 2;
        let (_, nav) = close_others(&mut tabs, &mut cur, 2, 3);
        assert!(nav);
        assert_eq!(tabs[cur], "b");
    }

    #[test]
    fn close_right_closes_only_unpinned_to_the_right() {
        let mut tabs = v(&["p1", "a", "b", "c"]);
        let mut cur = 3;
        let (removed, nav) = close_right(&mut tabs, &mut cur, 1, 1);
        assert_eq!(tabs, v(&["p1", "a"]));
        assert_eq!(removed, vec![("b".to_string(), 2), ("c".to_string(), 3)]);
        assert!(nav);
        assert_eq!(cur, 1);

        let mut tabs = v(&["p1", "a", "b", "c"]);
        let mut cur = 0;
        let (_, nav) = close_right(&mut tabs, &mut cur, 1, 1);
        assert!(!nav);
        assert_eq!(cur, 0);
    }

    #[test]
    fn close_right_of_pinned_tab_keeps_other_pinned() {
        let mut tabs = v(&["p1", "p2", "a", "b"]);
        let mut cur = 3;
        let (_, nav) = close_right(&mut tabs, &mut cur, 2, 0);
        assert_eq!(tabs, v(&["p1", "p2"]));
        assert!(nav);
        assert_eq!(cur, 0);
    }
}

#[cfg(test)]
mod start_page_tests {
    use super::*;
    use crate::config::DEFAULT_URL;

    #[test]
    fn start_page_is_recognized() {
        assert!(is_start_page(DEFAULT_URL, DEFAULT_URL));
        assert!(is_start_page(&format!("{}/", DEFAULT_URL), DEFAULT_URL));
        assert!(!is_start_page(&format!("{}/?q=rust", DEFAULT_URL), DEFAULT_URL));
        assert!(!is_start_page("https://example.com/", DEFAULT_URL));
    }
}
//! Вкладки: ярлыки, метаданные (уникальный id, звук, изолированная история переходов), проверка навигации и фильтр сообщений.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use crate::util::*;

/// Максимальная глубина истории «Назад / Вперёд» внутри одной вкладки.
pub(crate) const TAB_HIST_MAX: usize = 60;

pub(crate) fn next_tab_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn same_nav_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Изолированная история переходов («Назад / Вперёд») конкретной вкладки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabHist {
    pub(crate) entries: Vec<String>,
    pub(crate) cur: usize,
    /// Флаг активного программного перехода (переключение вкладки, шаг назад/вперёд),
    /// чтобы редирект или нормализация слеша не затирали ветку «Вперёд».
    pub(crate) in_step: bool,
}

impl TabHist {
    pub(crate) fn new(url: &str) -> Self {
        let mut entries = Vec::new();
        if !url.is_empty() {
            entries.push(url.to_string());
        }
        Self {
            entries,
            cur: 0,
            in_step: true,
        }
    }

    pub(crate) fn can_back(&self) -> bool {
        self.cur > 0
    }

    pub(crate) fn can_forward(&self) -> bool {
        self.cur + 1 < self.entries.len()
    }

    pub(crate) fn step_back(&mut self) -> Option<String> {
        if self.cur > 0 {
            self.cur -= 1;
            self.in_step = true;
            self.entries.get(self.cur).cloned()
        } else {
            None
        }
    }

    pub(crate) fn step_forward(&mut self) -> Option<String> {
        if self.cur + 1 < self.entries.len() {
            self.cur += 1;
            self.in_step = true;
            self.entries.get(self.cur).cloned()
        } else {
            None
        }
    }

    pub(crate) fn record(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if self.entries.is_empty() {
            self.entries.push(url.to_string());
            self.cur = 0;
            self.in_step = false;
            return;
        }
        if self.cur >= self.entries.len() {
            self.cur = self.entries.len() - 1;
        }
        if same_nav_url(&self.entries[self.cur], url) {
            self.entries[self.cur] = url.to_string();
            self.in_step = false;
            return;
        }
        if self.in_step {
            self.entries[self.cur] = url.to_string();
            self.in_step = false;
            return;
        }
        if self.cur > 0 && same_nav_url(&self.entries[self.cur - 1], url) {
            self.cur -= 1;
            self.entries[self.cur] = url.to_string();
            return;
        }
        if self.cur + 1 < self.entries.len() && same_nav_url(&self.entries[self.cur + 1], url) {
            self.cur += 1;
            self.entries[self.cur] = url.to_string();
            return;
        }
        self.entries.truncate(self.cur + 1);
        self.entries.push(url.to_string());
        if self.entries.len() > TAB_HIST_MAX {
            let drop_n = self.entries.len() - TAB_HIST_MAX;
            self.entries.drain(0..drop_n);
        }
        self.cur = self.entries.len() - 1;
    }
}

/// Метаданные вкладки: уникальный идентификатор (для связи с Live-WebView), состояние звука и история переходов.
#[derive(Clone, Debug)]
pub(crate) struct TabMeta {
    pub(crate) id: u64,
    pub(crate) muted: bool,
    pub(crate) hist: TabHist,
}

impl TabMeta {
    pub(crate) fn new(url: &str) -> Self {
        Self {
            id: next_tab_id(),
            muted: false,
            hist: TabHist::new(url),
        }
    }
}

/// Закрытая вкладка для возврата через Ctrl+Shift+T (вместе с её историей переходов).
#[derive(Clone, Debug)]
pub(crate) struct ClosedTab {
    pub(crate) url: String,
    pub(crate) index: usize,
    pub(crate) was_pinned: bool,
    pub(crate) hist: TabHist,
}

/// Гарантирует, что вектор метаданных совпадает по длине со списком вкладок.
pub(crate) fn ensure_metas(tabs: &[String], metas: &mut Vec<TabMeta>) {
    while metas.len() < tabs.len() {
        let u = &tabs[metas.len()];
        metas.push(TabMeta::new(u));
    }
    metas.truncate(tabs.len());
}

pub(crate) fn nav_state_js(metas: &[TabMeta], current_tab: usize) -> String {
    let (b, f) = metas
        .get(current_tab)
        .map(|m| (m.hist.can_back(), m.hist.can_forward()))
        .unwrap_or((false, false));
    format!(
        "if(window.__ulNavState) window.__ulNavState({},{});",
        if b { "true" } else { "false" },
        if f { "true" } else { "false" }
    )
}

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

/// Строка хостов для отображения иконок сайтов (favicon) на вкладках: "<хост1>|||<хост2>|||...".
pub(crate) fn tabs_hosts(tabs: &[String]) -> String {
    tabs.iter()
        .map(|u| host_of(u).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("|||")
}

// ---------- Иконки сайтов (favicon) ----------
// Каждая обычная вкладка - это новый документ, поэтому иконки нельзя хранить в JS: они пропадали при каждом
// переключении. Хранит их Rust: ключ - хост без "www.", значение - base64 PNG (не длиннее FAV_MAX_B64).
// Значение "!N" - "иконку получить не удалось" (N = число неудачных попыток, до 3; только на время сеанса).

pub(crate) const FAV_MAX_B64: usize = 7000;
pub(crate) const FAV_MAX_ENTRIES: usize = 300;

/// Ключ иконки сайта: хост без "www.".
pub(crate) fn fav_key(url: &str) -> Option<String> {
    let h = host_of(url)?;
    Some(h.strip_prefix("www.").map(|x| x.to_string()).unwrap_or(h))
}

/// Допустимое значение: base64 (PNG) или метка неудачи "!N".
pub(crate) fn fav_value_ok(v: &str) -> bool {
    if let Some(n) = v.strip_prefix('!') {
        return !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit());
    }
    !v.is_empty() && v.len() <= FAV_MAX_B64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// JS-вызов `__ulFavs('{"host":"<base64>",...}')` для перечисленных ключей (неизвестные пропускаются).
pub(crate) fn favs_js_for(favs: &HashMap<String, String>, keys: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        if let Some(v) = favs.get(k) {
            parts.push(format!("{}:{}", json_quote(k), json_quote(v)));
        }
    }
    let json = format!("{{{}}}", parts.join(","));
    format!("if(window.__ulFavs) window.__ulFavs('{}');", js_str(&json))
}

/// То же для всех сайтов, открытых во вкладках.
pub(crate) fn favs_js(favs: &HashMap<String, String>, tabs: &[String]) -> String {
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys: Vec<String> = Vec::new();
    for u in tabs {
        if let Some(k) = fav_key(u) {
            if seen.insert(k.clone()) {
                keys.push(k);
            }
        }
    }
    favs_js_for(favs, &keys)
}

/// Файл иконок: по строке "<хост>\t<base64>". Метки неудач на диск не пишем.
pub(crate) fn load_favs(path: &Path) -> HashMap<String, String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut m = HashMap::new();
    for l in text.lines() {
        if let Some((k, v)) = l.trim_end().split_once('\t') {
            if !v.starts_with('!') && fav_value_ok(v) && valid_domain(k) {
                m.insert(k.to_string(), v.to_string());
            }
        }
        if m.len() >= FAV_MAX_ENTRIES {
            break;
        }
    }
    m
}

pub(crate) fn save_favs(path: &Path, favs: &HashMap<String, String>) {
    let mut s = String::new();
    for (k, v) in favs {
        if !v.starts_with('!') {
            s.push_str(k);
            s.push('\t');
            s.push_str(v);
            s.push('\n');
        }
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
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
#[allow(dead_code)]
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

/// "Закрыть другие вкладки" вместе с синхронным обновлением `metas`.
pub(crate) fn close_others_meta(
    tabs: &mut Vec<String>,
    metas: &mut Vec<TabMeta>,
    current: &mut usize,
    pinned: usize,
    idx: usize,
) -> (Vec<ClosedTab>, bool) {
    ensure_metas(tabs, metas);
    let old_tabs = std::mem::take(tabs);
    let old_metas = std::mem::take(metas);
    let mut removed = Vec::new();
    let mut new_current = None;
    let mut new_idx = 0;
    for (i, (url, meta)) in old_tabs.into_iter().zip(old_metas.into_iter()).enumerate() {
        if i < pinned || i == idx {
            if i == *current {
                new_current = Some(tabs.len());
            }
            if i == idx {
                new_idx = tabs.len();
            }
            tabs.push(url);
            metas.push(meta);
        } else {
            removed.push(ClosedTab {
                url,
                index: i,
                was_pinned: false,
                hist: meta.hist,
            });
        }
    }
    let nav = new_current.is_none();
    *current = new_current.unwrap_or(new_idx);
    (removed, nav)
}

/// "Закрыть вкладки справа": закрываются все обычные вкладки правее `idx`.
/// Если текущая вкладка закрыта, текущей становится `idx` (и флаг `true`).
#[allow(dead_code)]
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

/// "Закрыть вкладки справа" вместе с синхронным обновлением `metas`.
pub(crate) fn close_right_meta(
    tabs: &mut Vec<String>,
    metas: &mut Vec<TabMeta>,
    current: &mut usize,
    pinned: usize,
    idx: usize,
) -> (Vec<ClosedTab>, bool) {
    ensure_metas(tabs, metas);
    let old_tabs = std::mem::take(tabs);
    let old_metas = std::mem::take(metas);
    let mut removed = Vec::new();
    for (i, (url, meta)) in old_tabs.into_iter().zip(old_metas.into_iter()).enumerate() {
        if i > idx && i >= pinned {
            removed.push(ClosedTab {
                url,
                index: i,
                was_pinned: false,
                hist: meta.hist,
            });
        } else {
            tabs.push(url);
            metas.push(meta);
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

    #[test]
    fn tab_hist_isolates_back_and_forward() {
        let mut h = TabHist::new("https://duckduckgo.com");
        // Первичная нормализация слеша при загрузке не должна создавать дубликат в истории
        h.record("https://duckduckgo.com/");
        assert!(!h.can_back());
        assert!(!h.can_forward());

        h.record("https://rust-lang.org/");
        assert!(h.can_back());
        assert!(!h.can_forward());

        assert_eq!(h.step_back().as_deref(), Some("https://duckduckgo.com/"));
        h.record("https://duckduckgo.com/");
        assert!(!h.can_back());
        assert!(h.can_forward());

        assert_eq!(h.step_forward().as_deref(), Some("https://rust-lang.org/"));
        h.record("https://rust-lang.org/");
        assert!(h.can_back());
        assert!(!h.can_forward());
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
        assert_eq!(tabs_hosts(&tabs), "www.example.com|||b.org");
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
mod fav_tests {
    use super::*;

    #[test]
    fn fav_key_drops_www() {
        assert_eq!(fav_key("https://www.Example.com/a").as_deref(), Some("example.com"));
        assert_eq!(fav_key("https://b.org:8080/").as_deref(), Some("b.org"));
        assert_eq!(fav_key("nope"), None);
    }

    #[test]
    fn fav_values_are_validated() {
        assert!(fav_value_ok("QUJD+/=="));
        assert!(fav_value_ok("!2"));
        assert!(!fav_value_ok(""));
        assert!(!fav_value_ok("!"));
        assert!(!fav_value_ok("a b"));
        assert!(!fav_value_ok("a'b"));
        assert!(!fav_value_ok(&"A".repeat(FAV_MAX_B64 + 1)));
    }

    #[test]
    fn favs_js_lists_each_tab_host_once() {
        let mut favs = HashMap::new();
        favs.insert("example.com".to_string(), "QUJD".to_string());
        favs.insert("other.org".to_string(), "!1".to_string());
        favs.insert("unused.net".to_string(), "QUJD".to_string());
        let tabs = vec![
            "https://www.example.com/a".to_string(),
            "https://example.com/b".to_string(),
            "https://other.org/".to_string(),
        ];
        let js = favs_js(&favs, &tabs);
        assert!(js.starts_with("if(window.__ulFavs) window.__ulFavs('{"));
        assert_eq!(js.matches("example.com").count(), 1);
        assert!(js.contains("\"other.org\":\"!1\""));
        assert!(!js.contains("unused.net"));
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
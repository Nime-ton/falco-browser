//! Подсказки в адресной строке: поиск по истории и закладкам.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use crate::history::*;
use crate::util::*;

/// Сколько подсказок показывать.
pub(crate) const SUGGEST_MAX: usize = 8;

/// Самая длинная принимаемая строка запроса (в символах).
const QUERY_MAX_CHARS: usize = 100;

pub(crate) struct Sug {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) bookmark: bool,
}

/// Кандидат: одна страница, склеенная из всех посещений и закладки.
pub(crate) struct Cand {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) bookmark: bool,
    pub(crate) visits: u32,
    pub(crate) last: u64,
}

/// Адрес без схемы и без "www." - в нём ищем и по нему сравниваем дубликаты.
fn bare_url(url_lower: &str) -> &str {
    let rest = url_lower.split_once("://").map(|x| x.1).unwrap_or(url_lower);
    rest.strip_prefix("www.").unwrap_or(rest)
}

fn score(c: &Cand, q: &str, tokens: &[&str], now: u64) -> Option<i64> {
    let url_l = c.url.to_lowercase();
    let bare = bare_url(&url_l);
    let title_l = c.title.to_lowercase();

    // каждое слово запроса должно встретиться в адресе или в заголовке
    for t in tokens {
        if !bare.contains(*t) && !title_l.contains(*t) {
            return None;
        }
    }

    let first = tokens[0];
    let host = bare.split(|ch| ch == '/' || ch == '?' || ch == '#').next().unwrap_or("");
    let mut s: i64 = 0;
    if host.starts_with(first) {
        s += 300;
    } else if host.contains(first) {
        s += 120;
    }
    if bare.starts_with(q) {
        s += 200;
    }
    if title_l.starts_with(q) {
        s += 120;
    } else if title_l.split(|ch: char| !ch.is_alphanumeric()).any(|w| w.starts_with(first)) {
        s += 60;
    }
    if c.bookmark {
        s += 150;
    }
    s += (c.visits.min(30) as i64) * 6;
    let age = now.saturating_sub(c.last);
    s += if age < 86_400 {
        60
    } else if age < 7 * 86_400 {
        35
    } else if age < 30 * 86_400 {
        15
    } else {
        0
    };
    // главные страницы (короткие адреса) чуть выше глубоких ссылок
    s -= (bare.len().min(200) as i64) / 8;
    Some(s)
}

/// Чистая функция без работы с файлами (её и тестируем): выбирает лучшие `limit` кандидатов.
pub(crate) fn rank(cands: Vec<Cand>, query: &str, now: u64, limit: usize) -> Vec<Sug> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || q.chars().count() > QUERY_MAX_CHARS {
        return Vec::new();
    }
    let tokens: Vec<&str> = q.split_whitespace().take(6).collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, Cand)> = cands
        .into_iter()
        .filter_map(|c| score(&c, &q, &tokens, now).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.last.cmp(&a.1.last)));

    // http/https-версии и адреса со слешем на конце считаем одной страницей
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (_, c) in scored {
        let lower = c.url.to_lowercase();
        let key = bare_url(&lower).trim_end_matches('/').to_string();
        if !seen.insert(key) {
            continue;
        }
        out.push(Sug { url: c.url, title: c.title, bookmark: c.bookmark });
        if out.len() >= limit {
            break;
        }
    }
    out
}

pub(crate) fn suggest(history: &Path, bookmarks: &Path, query: &str, limit: usize) -> Vec<Sug> {
    let mut map: HashMap<String, Cand> = HashMap::new();
    for e in read_entries(history) {
        let c = map.entry(e.url.clone()).or_insert_with(|| Cand {
            url: e.url.clone(),
            title: String::new(),
            bookmark: false,
            visits: 0,
            last: 0,
        });
        c.visits += 1;
        if e.ts >= c.last {
            c.last = e.ts;
            if !e.title.is_empty() {
                c.title = e.title;
            }
        }
    }
    for e in read_entries(bookmarks) {
        let c = map.entry(e.url.clone()).or_insert_with(|| Cand {
            url: e.url.clone(),
            title: String::new(),
            bookmark: false,
            visits: 0,
            last: 0,
        });
        c.bookmark = true;
        c.last = c.last.max(e.ts);
        if !e.title.is_empty() {
            c.title = e.title; // название закладки важнее заголовка страницы
        }
    }
    rank(map.into_values().collect(), query, now_secs(), limit)
}

/// JS-вызов для адресной строки: `__ulSug(<номер запроса>, <json>)`. Номер возвращается как есть,
/// чтобы страница отбросила запоздавший ответ.
pub(crate) fn suggest_js(seq: &str, list: &[Sug]) -> String {
    let mut o = String::from("[");
    for (i, s) in list.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!(
            "{{\"u\":{},\"t\":{},\"b\":{}}}",
            json_quote(&s.url),
            json_quote(&s.title),
            if s.bookmark { "true" } else { "false" }
        ));
    }
    o.push(']');
    format!("if(window.__ulSug) window.__ulSug('{}','{}');", seq, js_str(&o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(url: &str, title: &str, bookmark: bool, visits: u32, last: u64) -> Cand {
        Cand { url: url.to_string(), title: title.to_string(), bookmark, visits, last }
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn non_matching_pages_are_dropped() {
        let r = rank(vec![cand("https://a.com/", "Привет", false, 1, NOW)], "zzz", NOW, 8);
        assert!(r.is_empty());
        assert!(rank(vec![cand("https://a.com/", "x", false, 1, NOW)], "   ", NOW, 8).is_empty());
    }

    #[test]
    fn host_prefix_beats_title_match() {
        let r = rank(
            vec![
                cand("https://blog.org/post", "All about github", false, 1, NOW),
                cand("https://github.com/", "GitHub", false, 1, NOW),
            ],
            "git",
            NOW,
            8,
        );
        assert_eq!(r[0].url, "https://github.com/");
    }

    #[test]
    fn bookmark_is_preferred_over_plain_history() {
        let r = rank(
            vec![
                cand("https://rust-lang.org/a", "Rust", false, 1, NOW),
                cand("https://rust-lang.org/b", "Rust", true, 1, NOW),
            ],
            "rust",
            NOW,
            8,
        );
        assert_eq!(r[0].url, "https://rust-lang.org/b");
        assert!(r[0].bookmark);
    }

    #[test]
    fn all_words_must_match_and_case_is_ignored() {
        let c = || cand("https://example.com/", "Мой Любимый Сайт", false, 1, NOW);
        assert_eq!(rank(vec![c()], "любимый сайт", NOW, 8).len(), 1);
        assert_eq!(rank(vec![c()], "любимый EXAMPLE", NOW, 8).len(), 1);
        assert!(rank(vec![c()], "любимый другой", NOW, 8).is_empty());
    }

    #[test]
    fn http_https_and_trailing_slash_are_one_page() {
        let r = rank(
            vec![
                cand("http://www.example.com", "Example", false, 1, NOW - 100),
                cand("https://example.com/", "Example", false, 3, NOW),
            ],
            "example",
            NOW,
            8,
        );
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn limit_is_respected() {
        let cands: Vec<Cand> = (0..20).map(|i| cand(&format!("https://site{}.com/", i), "site", false, 1, NOW)).collect();
        assert_eq!(rank(cands, "site", NOW, 5).len(), 5);
    }

    #[test]
    fn js_output_is_escaped() {
        let list = vec![Sug { url: "https://a.com/?q='x'".to_string(), title: "It's \"x\"".to_string(), bookmark: true }];
        let js = suggest_js("7", &list);
        assert!(js.starts_with("if(window.__ulSug) window.__ulSug('7','"));
        assert!(js.contains("\\'x\\'"));
        assert!(!js.contains('\n'));
    }
}
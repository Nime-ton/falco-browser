//! Блокировщик рекламы: разбор списков фильтров (hosts, ||domain^, косметические правила).

use std::fs;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use crate::util::*;

pub(crate) const AD_HOSTS: &[&str] = &[
    "amazon-adsystem.com", "adnxs.com", "criteo.com", "criteo.net", "taboola.com", "outbrain.com", "mgid.com", "adskeeper.com",
    "exoclick.com", "clickadu.com", "trafficstars.com", "propellerads.com", "popads.net", "a-ads.com", "adsterra.com",
    "highperformanceformat.com", "popcash.net", "onclickads.net", "juicyads.com", "adcash.com", "hilltopads.net",
    "rubiconproject.com", "openx.net", "pubmatic.com", "smartadserver.com", "moatads.com", "adform.net", "adtelligent.com",
    "bidswitch.net", "casalemedia.com", "adsrvr.org",
    "an.yandex.ru", "mc.yandex.ru", "awaps.yandex.net", "yandexadexchange.net", "adfox.ru", "adriver.ru",
    "ad.mail.ru", "top-fwz1.mail.ru", "directadvert.ru", "begun.ru", "marketgid.com", "tns-counter.ru", "yadro.ru",
    "betweendigital.com",
];

pub(crate) const GOOGLE_AD_HOSTS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "adservice.google.com", "google-analytics.com",
];

pub(crate) const NEVER_BLOCK: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "googlevideo.com", "ytimg.com", "ggpht.com",
    "gstatic.com", "google.com", "googleapis.com", "googleusercontent.com", "gvt1.com", "gvt2.com",
];

pub(crate) const NO_DNS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "google-analytics.com",
    "googletagmanager.com", "googletagservices.com", "2mdn.net", "adservice.google.com",
];

pub(crate) const DNS_BUDGET: usize = 20_000;

pub(crate) const MAX_JS_HOSTS: usize = 15_000;

pub(crate) const MAX_GENERIC_COSMETIC: usize = 3_000;

pub(crate) const MAX_HOST_COSMETIC: usize = 6_000;

#[derive(Default)]
pub(crate) struct Filters {
    pub(crate) dns: Vec<String>,
    pub(crate) js: Vec<String>,
    pub(crate) generic: Vec<String>,
    pub(crate) per_host: HashMap<String, Vec<String>>,
}

pub(crate) fn has_ancestor(set: &HashSet<String>, host: &str) -> bool {
    let mut h = host;
    loop {
        if set.contains(h) {
            return true;
        }
        match h.find('.') {
            Some(i) => h = &h[i + 1..],
            None => return false,
        }
    }
}

pub(crate) fn parse_network_rule(line: &str) -> Option<(String, bool)> {
    let (is_exc, rest) = match line.strip_prefix("@@") {
        Some(r) => (true, r),
        None => (false, line),
    };
    let rest = rest.strip_prefix("||")?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    let domain = rest[..end].to_ascii_lowercase();
    let tail = &rest[end..];
    let opts: &str = if tail.is_empty() || tail == "^" {
        ""
    } else if let Some(o) = tail.strip_prefix("^$") {
        o
    } else if let Some(o) = tail.strip_prefix('$') {
        o
    } else {
        return None;
    };
    for o in opts.split(',') {
        let ok = match o {
            "" | "important" | "all" => true,
            // Блокирующее правило "только для третьих сторон" нельзя превращать в блокировку
            // всего домена (иначе сам сайт перестанет открываться). Исключения ослабляют блокировку - им можно.
            "third-party" | "3p" => is_exc,
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    if !valid_domain(&domain) {
        return None;
    }
    Some((domain, is_exc))
}

pub(crate) fn parse_hosts_line(line: &str) -> Option<String> {
    let mut it = line.split_whitespace();
    let first = it.next()?;
    let d = match first {
        "0.0.0.0" | "127.0.0.1" => it.next()?,
        _ => {
            if it.next().is_some() {
                return None;
            }
            first
        }
    };
    let d = d.to_ascii_lowercase();
    if valid_domain(&d) {
        Some(d)
    } else {
        None
    }
}

pub(crate) fn parse_cosmetic(line: &str) -> Option<(Vec<String>, String)> {
    const BAD: &[&str] = &[
        ":-abp-", ":has-text", ":contains(", ":matches-css", ":xpath", ":upward", ":remove(", ":style(",
        "+js(", ":nth-ancestor", ":min-text", ":watch-attr", ":others(",
    ];
    let idx = line.find("##")?;
    let left = &line[..idx];
    let sel = line[idx + 2..].trim();
    if sel.is_empty() || sel.len() > 200 {
        return None;
    }
    if sel.contains(|c: char| matches!(c, '{' | '}' | ';')) || BAD.iter().any(|b| sel.contains(*b)) {
        return None;
    }
    let mut doms = Vec::new();
    if !left.is_empty() {
        for d in left.split(',') {
            let d = d.trim().to_ascii_lowercase();
            if d.starts_with('~') || !valid_domain(&d) {
                return None;
            }
            doms.push(d);
        }
    }
    Some((doms, sel.to_string()))
}

pub(crate) fn load_filters(dir: &Path) -> Filters {
    // 1. Проверяем, существует ли папка или пуста ли она (нет файлов .txt)
    let txt_files = list_files(dir, "txt");
    if txt_files.is_empty() {
        let _ = fs::create_dir_all(dir);
        let default_file = dir.join("block.txt");
        // Зашитая строка с дефолтными правилами блокировки
        let default_rules = 
            "! Встроенные дефолтные фильтры Falco\n\
             ||doubleclick.net^\n\
             ||googlesyndication.com^\n\
             ||googleadservices.com^\n\
             ||adservice.google.com^\n\
             ||mc.yandex.ru^\n\
             ||an.yandex.ru^\n\
             ||amazon-adsystem.com^\n\
             ||criteo.com^\n\
             ||taboola.com^\n\
             ||outbrain.com^\n\
             ||propellerads.com^\n\
             ||popads.net^\n";
        let _ = fs::write(&default_file, default_rules);
    }

    let mut blocked: Vec<String> = Vec::new();
    let mut allowed: HashSet<String> = HashSet::new();
    let mut generic: Vec<String> = Vec::new();
    let mut per_host: HashMap<String, Vec<String>> = HashMap::new();
    let mut host_count = 0usize;

    for path in list_files(dir, "txt") {
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
                continue;
            }
            if line.contains("##") {
                if let Some((doms, sel)) = parse_cosmetic(line) {
                    if doms.is_empty() {
                        if generic.len() < MAX_GENERIC_COSMETIC {
                            generic.push(sel);
                        }
                    } else if host_count < MAX_HOST_COSMETIC {
                        for d in doms {
                            per_host.entry(d).or_default().push(sel.clone());
                            host_count += 1;
                        }
                    }
                }
                continue;
            }
            if line.starts_with("||") || line.starts_with("@@") {
                if let Some((d, exc)) = parse_network_rule(line) {
                    if exc {
                        allowed.insert(d);
                    } else {
                        blocked.push(d);
                    }
                }
                continue;
            }
            if let Some(d) = parse_hosts_line(line) {
                blocked.push(d);
            }
        }
    }

    let mut f = Filters::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut dns_len = 0usize;
    let items = AD_HOSTS
        .iter()
        .map(|s| (s.to_string(), true))
        .chain(blocked.into_iter().map(|d| (d, false)));
    for (d, builtin) in items {
        if has_ancestor(&seen, &d) {
            continue;
        }
        if !builtin && (allowed.contains(&d) || is_under_any(&d, NEVER_BLOCK)) {
            continue;
        }
        seen.insert(d.clone());
        let cost = d.len() * 2 + 40;
        if !is_under_any(&d, NO_DNS) && dns_len + cost <= DNS_BUDGET {
            dns_len += cost;
            f.dns.push(d);
        } else if f.js.len() < MAX_JS_HOSTS {
            f.js.push(d);
        }
    }
    for h in AD_HOSTS.iter().chain(GOOGLE_AD_HOSTS.iter()) {
        f.js.push(h.to_string());
    }
    f.generic = generic;
    f.per_host = per_host;
    f
}

pub(crate) fn cosmetic_json(f: &Filters) -> String {
    let mut hosts: Vec<(&String, &Vec<String>)> = f.per_host.iter().collect();
    hosts.sort_by(|a, b| a.0.cmp(b.0));
    let h = hosts
        .iter()
        .map(|(k, v)| format!("{}:{}", json_str(k), json_arr(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"g\":{},\"h\":{{{}}}}}", json_arr(&f.generic), h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_block_rules_are_rejected() {
        assert_eq!(parse_network_rule("||example.com^$third-party"), None);
        assert_eq!(parse_network_rule("||example.com^$3p,important"), None);
    }

    #[test]
    fn plain_and_exception_rules_are_accepted() {
        assert_eq!(parse_network_rule("||example.com^"), Some(("example.com".to_string(), false)));
        assert_eq!(parse_network_rule("||example.com^$important"), Some(("example.com".to_string(), false)));
        assert_eq!(parse_network_rule("@@||example.com^$third-party"), Some(("example.com".to_string(), true)));
    }

    #[test]
    fn unsupported_rules_are_rejected() {
        assert_eq!(parse_network_rule("||example.com/path"), None);
        assert_eq!(parse_network_rule("||example.com^$script"), None);
    }
}
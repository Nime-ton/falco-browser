//! Блокировщик рекламы: разбор списков фильтров (hosts, ||domain^, косметические правила), белый список (whitelist.txt) и онлайн-обновление.

use std::fs;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use crate::util::*;

pub(crate) const WHITELIST_EXAMPLE: &str = "# Белый список сайтов (кнопка «Щит» в адресной строке).\n# На этих доменах (и их поддоменах) блокировщик рекламы отключён.\n# По одному домену в строке.\n";

pub(crate) fn load_whitelist(path: &Path) -> HashSet<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && valid_domain(l))
        .collect()
}

pub(crate) fn save_whitelist(path: &Path, set: &HashSet<String>) {
    let mut list: Vec<&String> = set.iter().collect();
    list.sort();
    let mut s = String::from(WHITELIST_EXAMPLE);
    for d in list {
        s.push_str(d);
        s.push('\n');
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

pub(crate) fn is_whitelisted_host(host: &str, wl: &HashSet<String>) -> bool {
    let h = host.to_ascii_lowercase();
    has_ancestor(wl, &h)
}

pub(crate) fn is_whitelisted_url(url: &str, wl: &HashSet<String>) -> bool {
    match host_of(url) {
        Some(h) => is_whitelisted_host(&h, wl),
        None => false,
    }
}

/// Переключает статус сайта в белом списке.
/// Если поддомен уже покрыт родительским доменом в списке — удаляет родительский домен, иначе добавляет базовый хост.
/// Возвращает Some(true), если сайт теперь в белом списке (защита выключена), Some(false) — если удалён (защита включена).
pub(crate) fn toggle_whitelist(path: &Path, wl: &mut HashSet<String>, url: &str) -> Option<bool> {
    let host = host_of(url)?;
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if !valid_domain(&host) {
        return None;
    }
    let mut matching = Vec::new();
    let mut h = host.as_str();
    loop {
        if wl.contains(h) {
            matching.push(h.to_string());
        }
        match h.find('.') {
            Some(i) => h = &h[i + 1..],
            None => break,
        }
    }
    let now_whitelisted = if !matching.is_empty() {
        for m in matching {
            wl.remove(&m);
        }
        false
    } else {
        wl.insert(host);
        true
    };
    save_whitelist(path, wl);
    Some(now_whitelisted)
}

pub(crate) fn whitelist_csv(wl: &HashSet<String>) -> String {
    let mut v: Vec<&str> = wl.iter().map(|s| s.as_str()).collect();
    v.sort_unstable();
    v.join(",")
}

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

/// Списки фильтров, вшитые в exe при сборке (файлы лежат в <проект>/lists/).
/// Без них exe на чужом ПК получал только короткий встроенный набор правил.
const EMBEDDED_LISTS: &[(&str, &[u8])] = &[
    ("easylist.txt", include_bytes!("../lists/easylist.txt")),
    ("easyprivacy.txt", include_bytes!("../lists/easyprivacy.txt")),
];

/// Официальные URL для обновления списков фильтров по кнопке в Настройках.
/// Списки для онлайн-обновления: файл в папке filters и адреса по порядку (если первый недоступен, пробуем зеркало).
pub(crate) const FILTER_SOURCES: &[(&str, &[&str])] = &[
    (
        "easylist.txt",
        &["https://easylist.to/easylist/easylist.txt", "https://secure.fanboy.co.nz/easylist.txt"],
    ),
    (
        "easyprivacy.txt",
        &["https://easylist.to/easylist/easyprivacy.txt", "https://secure.fanboy.co.nz/easyprivacy.txt"],
    ),
];

/// Номер версии из заголовка списка ("! Version: 202610222129") или None.
fn list_version(head: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(head);
    for line in text.lines().take(20) {
        if let Some(v) = line.trim().strip_prefix("! Version:") {
            return v.trim().parse::<u64>().ok();
        }
    }
    None
}

/// Версия списка, лежащего на диске (читаем только начало файла).
fn file_version(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut buf = Vec::new();
    fs::File::open(path).ok()?.take(4096).read_to_end(&mut buf).ok()?;
    list_version(&buf)
}

/// Раскладывает вшитые списки в папку фильтров.
/// - Файла нет -> создаётся.
/// - Файл есть и в нём указана версия СТАРШЕ вшитой -> заменяется новой (так списки обновляются
///   вместе с exe). Свои правила держите в отдельном файле (например my.txt): easylist.txt и
///   easyprivacy.txt могут быть перезаписаны. Если у файла нет версии или она не старше - он не трогается.
/// - Есть `_easylist.txt` (список отключён переименованием, файлы с `_` игнорируются) -> не создаётся.
pub(crate) fn seed_embedded_lists(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    for (name, data) in EMBEDDED_LISTS {
        let data: &[u8] = data;
        let active = dir.join(name);
        if dir.join(format!("_{}", name)).exists() {
            continue;
        }
        let need = if active.exists() {
            match (file_version(&active), list_version(&data[..data.len().min(4096)])) {
                (Some(old), Some(new)) => new > old,
                _ => false,
            }
        } else {
            true
        };
        if !need {
            continue;
        }
        // сначала во временный файл (расширение не .txt), чтобы недописанный список не загрузился
        let tmp = dir.join(format!("{}.tmp", name));
        if fs::write(&tmp, data).is_ok() && fs::rename(&tmp, &active).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

fn download_url_to_file(url: &str, dest: &Path) -> Result<(), String> {
    let dest_str = dest.to_str().ok_or_else(|| "Некорректный путь к файлу".to_string())?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let curl_ok = std::process::Command::new("curl.exe")
            .args(["-fsSL", "--max-time", "45", "-A", "Mozilla/5.0 Falco", "-o", dest_str, url])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok {
            return Ok(());
        }

        let ps_cmd = format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing -TimeoutSec 45",
            url,
            dest_str.replace('\'', "''")
        );
        let ps_ok = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ps_ok {
            Ok(())
        } else {
            Err(format!("Не удалось скачать {}", url))
        }
    }

    #[cfg(not(windows))]
    {
        let ok = std::process::Command::new("curl")
            .args(["-fsSL", "--max-time", "45", "-A", "Mozilla/5.0 Falco", "-o", dest_str, url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err(format!("Не удалось скачать {}", url))
        }
    }
}

/// Пробует адреса по очереди, пока один не скачается.
fn download_first(urls: &[&str], dest: &Path) -> Result<(), String> {
    let mut last = String::from("Нет адресов для скачивания");
    for u in urls {
        match download_url_to_file(u, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = fs::remove_file(dest);
                last = e;
            }
        }
    }
    Err(last)
}

/// Скачивает свежие списки фильтров (easylist.txt, easyprivacy.txt) в папку `dir`.
/// Возвращает количество обновлённых списков.
pub(crate) fn update_filter_lists(dir: &Path) -> Result<usize, String> {
    let _ = fs::create_dir_all(dir);
    let mut updated = 0usize;
    let mut last_err = None;

    for (name, urls) in FILTER_SOURCES {
        if dir.join(format!("_{}", name)).exists() {
            continue;
        }
        let tmp = dir.join(format!("{}.download.tmp", name));
        let _ = fs::remove_file(&tmp);
        match download_first(urls, &tmp) {
            Ok(()) => {
                let bytes = match fs::read(&tmp) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = fs::remove_file(&tmp);
                        last_err = Some(e.to_string());
                        continue;
                    }
                };
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
                let head_trim = head.trim_start_matches('\u{feff}').trim_start();
                if bytes.len() < 10_000 || !(head_trim.starts_with("[Adblock") || head_trim.starts_with('!')) {
                    let _ = fs::remove_file(&tmp);
                    last_err = Some(format!("Файл {} повреждён или не является списком фильтров", name));
                    continue;
                }
                let active = dir.join(name);
                if fs::rename(&tmp, &active).is_ok() {
                    updated += 1;
                } else {
                    let _ = fs::remove_file(&tmp);
                    last_err = Some(format!("Не удалось сохранить {}", name));
                }
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                last_err = Some(e);
            }
        }
    }

    if updated > 0 {
        Ok(updated)
    } else {
        Err(last_err.unwrap_or_else(|| "Все списки отключены".to_string()))
    }
}

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
    } else {
        // Глобальные правила с поиском подстрок в атрибутах ([class*="..."], [id^="..."])
        // заставляют движок браузера проверять строки на каждом узле при стриминге DOM
        if sel.contains("*=") || sel.contains("^=") || sel.contains("$=") {
            return None;
        }
    }
    Some((doms, sel.to_string()))
}

pub(crate) fn load_filters(dir: &Path) -> Filters {
    // Папку и списки создаёт seed_embedded_lists (main.rs). Если списков нет совсем,
    // работают встроенные AD_HOSTS.
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

    #[test]
    fn whitelist_toggle_works_for_subdomains() {
        let dir = tmp_dir("wl");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("whitelist.txt");
        let mut wl = HashSet::new();
        assert_eq!(toggle_whitelist(&path, &mut wl, "https://www.example.com/pay"), Some(true));
        assert!(is_whitelisted_url("https://sub.example.com/a", &wl));
        assert_eq!(toggle_whitelist(&path, &mut wl, "https://sub.example.com/a"), Some(false));
        assert!(!is_whitelisted_url("https://example.com/", &wl));
        let _ = fs::remove_dir_all(&dir);
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("falco-filters-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn list_version_is_parsed() {
        assert_eq!(list_version(b"[Adblock Plus 2.0]\r\n! Version: 202610222129\r\n! Title: X"), Some(202610222129));
        assert_eq!(list_version(b"||a.com^\n"), None);
    }

    #[test]
    fn seed_creates_updates_and_respects_user_choices() {
        let dir = tmp_dir("seed");
        seed_embedded_lists(&dir);
        assert!(dir.join("easylist.txt").exists());
        assert!(dir.join("easyprivacy.txt").exists());

        // старая версия заменяется вшитой
        fs::write(dir.join("easylist.txt"), "! Version: 1\n||old.example^\n").unwrap();
        seed_embedded_lists(&dir);
        assert!(fs::metadata(dir.join("easylist.txt")).unwrap().len() > 1000);

        // файл без версии (самодельный) не трогаем
        fs::write(dir.join("easyprivacy.txt"), "||mine.example^\n").unwrap();
        seed_embedded_lists(&dir);
        assert_eq!(fs::read_to_string(dir.join("easyprivacy.txt")).unwrap(), "||mine.example^\n");

        // отключённый список не возвращается
        fs::remove_file(dir.join("easylist.txt")).unwrap();
        fs::write(dir.join("_easylist.txt"), "x").unwrap();
        seed_embedded_lists(&dir);
        assert!(!dir.join("easylist.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
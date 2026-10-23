#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

// Рекламные и трекерные домены. Список используется дважды:
// 1) в JS (fetch/XHR/script/iframe/img);
// 2) на Windows - как правила DNS движка WebView2 (режет вообще любые запросы к ним).
const AD_HOSTS: &[&str] = &[
    "amazon-adsystem.com", "adnxs.com", "criteo.com", "criteo.net", "taboola.com", "outbrain.com", "mgid.com", "adskeeper.com",
    "exoclick.com", "clickadu.com", "trafficstars.com", "propellerads.com", "popads.net", "a-ads.com", "adsterra.com",
    "highperformanceformat.com", "popcash.net", "onclickads.net", "juicyads.com", "adcash.com", "hilltopads.net",
    "rubiconproject.com", "openx.net", "pubmatic.com", "smartadserver.com", "moatads.com", "adform.net", "adtelligent.com",
    "bidswitch.net", "casalemedia.com", "adsrvr.org",
    "an.yandex.ru", "mc.yandex.ru", "awaps.yandex.net", "yandexadexchange.net", "adfox.ru", "adriver.ru",
    "ad.mail.ru", "top-fwz1.mail.ru", "directadvert.ru", "begun.ru", "marketgid.com", "tns-counter.ru", "yadro.ru",
    "betweendigital.com",
];

// Домены Google: НЕ режем на уровне DNS, иначе YouTube видит сетевые ошибки рекламных запросов
// и наказывает бесконечной буферизацией. Блокируются только в JS и только на сайтах, кроме YouTube.
const GOOGLE_AD_HOSTS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "adservice.google.com", "google-analytics.com",
];

// ====================== Фильтры и пользовательские скрипты ======================
// Папка данных: %LOCALAPPDATA%\Falco\  (рядом с "User Data")
//   filters\*.txt   - списки фильтров (hosts, EasyList / AdGuard / uBlock). Файлы на "_" пропускаются.
//   scripts\*.js    - пользовательские скрипты формата Tampermonkey (@match/@include/@exclude/@run-at/@noframes).
//                     Файлы на "_" пропускаются.

// Эти домены (и их поддомены) из списков не блокируются никогда.
const NEVER_BLOCK: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "googlevideo.com", "ytimg.com", "ggpht.com",
    "gstatic.com", "google.com", "googleapis.com", "googleusercontent.com", "gvt1.com", "gvt2.com",
];
// Рекламные домены Google: на уровне DNS не режем (YouTube реагирует на это буферизацией),
// блокируются только скриптом и только вне YouTube.
const NO_DNS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "google-analytics.com",
    "googletagmanager.com", "googletagservices.com", "2mdn.net", "adservice.google.com",
];
// Лимит командной строки Windows ~32 КБ, поэтому в DNS-правила попадает только часть доменов.
const DNS_BUDGET: usize = 20_000;
// Остальные домены проверяются в JS по хеш-таблице. Лимит нужен, чтобы скрипт не раздувался.
const MAX_JS_HOSTS: usize = 15_000;
const MAX_GENERIC_COSMETIC: usize = 3_000;
const MAX_HOST_COSMETIC: usize = 6_000;

#[derive(Default)]
struct Filters {
    dns: Vec<String>,
    js: Vec<String>,
    generic: Vec<String>,
    per_host: HashMap<String, Vec<String>>,
}

fn is_under(host: &str, base: &str) -> bool {
    host == base
        || (host.len() > base.len()
            && host.ends_with(base)
            && host.as_bytes()[host.len() - base.len() - 1] == b'.')
}

fn is_under_any(host: &str, list: &[&str]) -> bool {
    list.iter().any(|b| is_under(host, b))
}

fn has_ancestor(set: &HashSet<String>, host: &str) -> bool {
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

fn valid_domain(d: &str) -> bool {
    let n = d.len();
    if n < 4 || n > 253 || !d.contains('.') || d.starts_with('.') || d.ends_with('.') || d.starts_with('-') || d.contains("..") {
        return false;
    }
    if !d.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') {
        return false;
    }
    // IP-адреса не домены
    !d.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

// ||domain^  /  ||domain^$third-party  /  @@||domain^  - остальные (с путями, регэкспами, типами) пропускаем
fn parse_network_rule(line: &str) -> Option<(String, bool)> {
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
        if !o.is_empty() && !matches!(o, "third-party" | "3p" | "important" | "all") {
            return None;
        }
    }
    if !valid_domain(&domain) {
        return None;
    }
    Some((domain, is_exc))
}

// "0.0.0.0 domain", "127.0.0.1 domain" или просто "domain"
fn parse_hosts_line(line: &str) -> Option<String> {
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

// "##selector" и "domain1,domain2##selector". Расширенные синтаксисы (:has-text, +js и т.п.) пропускаем.
fn parse_cosmetic(line: &str) -> Option<(Vec<String>, String)> {
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

fn list_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
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

fn load_filters(dir: &Path) -> Filters {
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
    // Встроенные списки дублируем в JS (на случай, если DNS-правила не сработают)
    for h in AD_HOSTS.iter().chain(GOOGLE_AD_HOSTS.iter()) {
        f.js.push(h.to_string());
    }
    f.generic = generic;
    f.per_host = per_host;
    f
}

fn json_str(s: &str) -> String {
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

fn json_arr(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}

fn cosmetic_json(f: &Filters) -> String {
    let mut hosts: Vec<(&String, &Vec<String>)> = f.per_host.iter().collect();
    hosts.sort_by(|a, b| a.0.cmp(b.0));
    let h = hosts
        .iter()
        .map(|(k, v)| format!("{}:{}", json_str(k), json_arr(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"g\":{},\"h\":{{{}}}}}", json_arr(&f.generic), h)
}

// Каждый пользовательский скрипт - ОТДЕЛЬНЫЙ initialization script: синтаксическая ошибка
// в одном файле не ломает ни остальные скрипты, ни сам браузер.
const USER_SCRIPT_TEMPLATE: &str = r##"(function () {
    try {
        var M = __M__, I = __I__, E = __E__, EX = __EX__, RT = "__RT__", NF = __NF__;
        if (NF && window !== window.top) return;
        var esc = function (s) { return s.replace(/[.+?^${}()|[\]\\]/g, '\\$&'); };
        var g2r = function (s) {
            if (s.length > 2 && s.charAt(0) === '/' && s.charAt(s.length - 1) === '/') return new RegExp(s.slice(1, -1), 'i');
            return new RegExp('^' + esc(s).replace(/\*/g, '.*') + '$', 'i');
        };
        var m2r = function (p) {
            if (p === '<all_urls>') return /^(https?|file):\/\//i;
            var m = /^(\*|https?|file):\/\/([^\/]*)(\/.*)?$/.exec(p);
            if (!m) return g2r(p);
            var h = m[2], hr;
            if (h === '*') hr = '[^/]+';
            else if (h.indexOf('*.') === 0) hr = '(?:[^/]+\\.)?' + esc(h.slice(2));
            else hr = esc(h).replace(/\*/g, '[^/]*');
            return new RegExp('^' + (m[1] === '*' ? 'https?' : m[1]) + ':\\/\\/' + hr + esc(m[3] || '/').replace(/\*/g, '.*') + '$', 'i');
        };
        var any = function (list) {
            for (var i = 0; i < list.length; i++) if (list[i].test(location.href)) return true;
            return false;
        };
        var inc = M.map(m2r).concat(I.map(g2r)), exc = EX.map(m2r).concat(E.map(g2r));
        if (!inc.length || !any(inc) || any(exc)) return;

        var NAME = __NAMEJSON__, P = 'ulgm:' + NAME + ':';
        var GM_getValue = function (k, d) { try { var v = localStorage.getItem(P + k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } };
        var GM_setValue = function (k, v) { try { localStorage.setItem(P + k, JSON.stringify(v)); } catch (e) {} };
        var GM_deleteValue = function (k) { try { localStorage.removeItem(P + k); } catch (e) {} };
        var GM_addStyle = function (css) {
            var s = document.createElement('style'); s.textContent = css;
            (document.head || document.documentElement).appendChild(s); return s;
        };
        var GM_log = function () { console.log.apply(console, arguments); };
        var GM_registerMenuCommand = function () {};
        var GM_setClipboard = function (t) { try { navigator.clipboard.writeText(String(t)); } catch (e) {} };
        var GM_info = { script: { name: NAME }, scriptHandler: 'Falco' };
        var GM = {
            getValue: function (k, d) { return Promise.resolve(GM_getValue(k, d)); },
            setValue: function (k, v) { GM_setValue(k, v); return Promise.resolve(); },
            deleteValue: function (k) { GM_deleteValue(k); return Promise.resolve(); },
            addStyle: function (c) { return Promise.resolve(GM_addStyle(c)); },
            log: GM_log, info: GM_info
        };

        var run = function () {
            try {
                (function (unsafeWindow, GM_getValue, GM_setValue, GM_deleteValue, GM_addStyle, GM_log, GM_registerMenuCommand, GM_setClipboard, GM_info, GM) {
__CODE__
                }).call(window, window, GM_getValue, GM_setValue, GM_deleteValue, GM_addStyle, GM_log, GM_registerMenuCommand, GM_setClipboard, GM_info, GM);
            } catch (e) { console.error('[Falco userscript] ' + NAME, e); }
        };
        var go = RT === 'document-idle' ? function () { setTimeout(run, 0); } : run;
        if (RT === 'document-start') run();
        else if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', go, { once: true });
        else go();
    } catch (e) {}
})();
"##;

const EXAMPLE_SCRIPT: &str = r##"// ==UserScript==
// @name        Пример (файлы, имя которых начинается с _, игнорируются - уберите подчёркивание)
// @match       http://127.0.0.1:7860/*
// @run-at      document-idle
// ==/UserScript==
GM_addStyle('footer { display: none !important; }');
"##;

fn load_user_scripts(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for path in list_files(dir, "js") {
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let code = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string();
        let mut name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("script").to_string();
        let mut run_at: &str = "document-idle";
        let mut noframes = false;
        let (mut m, mut i, mut e, mut ex): (Vec<String>, Vec<String>, Vec<String>, Vec<String>) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut in_header = false;
        for line in code.lines() {
            let l = line.trim();
            if l.starts_with("// ==UserScript==") {
                in_header = true;
                continue;
            }
            if l.starts_with("// ==/UserScript==") {
                break;
            }
            if !in_header {
                continue;
            }
            let l = match l.strip_prefix("//") {
                Some(r) => r.trim_start(),
                None => continue,
            };
            let l = match l.strip_prefix('@') {
                Some(r) => r,
                None => continue,
            };
            let (key, val) = match l.split_once(char::is_whitespace) {
                Some((k, v)) => (k, v.trim()),
                None => (l, ""),
            };
            match key {
                "name" if !val.is_empty() => name = val.to_string(),
                "match" => m.push(val.to_string()),
                "include" => i.push(val.to_string()),
                "exclude" => e.push(val.to_string()),
                "exclude-match" => ex.push(val.to_string()),
                "run-at" => {
                    run_at = match val {
                        "document-start" => "document-start",
                        "document-end" | "document-body" => "document-end",
                        _ => "document-idle",
                    }
                }
                "noframes" => noframes = true,
                _ => {}
            }
        }
        if m.is_empty() && i.is_empty() {
            continue; // без @match/@include скрипт нигде не запускается (как в Tampermonkey)
        }
        let js = USER_SCRIPT_TEMPLATE
            .replace("__M__", &json_arr(&m))
            .replace("__I__", &json_arr(&i))
            .replace("__EX__", &json_arr(&ex))
            .replace("__E__", &json_arr(&e))
            .replace("__NAMEJSON__", &json_str(&name))
            .replace("__RT__", run_at)
            .replace("__NF__", if noframes { "true" } else { "false" })
            .replace("__CODE__", &code);
        out.push(js);
    }
    out
}

// ====================== Нативный блокировщик (feature "native-adblock") ======================
// Сборка: cargo build --release --features native-adblock
// Движок adblock (Brave) читает те же filters\*.txt (EasyList/uBlock/AdGuard) и проверяет КАЖДЫЙ запрос
// страницы с учётом пути, типа ресурса ($script, $image, $xmlhttprequest...), $third-party и $domain=.
// Запросы блокируются через WebView2 WebResourceRequested. Ограничения: проверяются запросы основного
// документа (iframe и воркеры закрывают DNS и JS-слои), документы (навигация) не блокируются,
// на YouTube и на 127.0.0.1/localhost движок отключён.
#[cfg(all(windows, feature = "native-adblock"))]
mod native_adblock {
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    use adblock::lists::{FilterSet, ParseOptions};
    use adblock::request::Request;
    use adblock::Engine;
    use webview2_com::take_pwstr;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_2, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    };
    use webview2_com::WebResourceRequestedEventHandler;
    use windows::core::{ComInterface, HSTRING, PWSTR};
    use windows::Win32::System::WinRT::EventRegistrationToken;
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use wry::WebViewExtWindows;

    // Лог диагностики (%LOCALAPPDATA%\Falco\native-adblock-debug.log). Выключен: запись файла
    // из обработчика запроса тормозит загрузку. Поставьте true, только когда ищете проблему.
    pub const DEBUG: bool = false;

    // Дописывает строку в лог диагностики
    pub fn log(path: &Path, line: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    pub fn build_engine(dir: &Path) -> (Engine, usize) {
        let mut lines: Vec<String> = Vec::new();
        for path in super::list_files(dir, "txt") {
            if let Ok(bytes) = std::fs::read(&path) {
                for l in String::from_utf8_lossy(&bytes).lines() {
                    let l = l.trim();
                    // Движку нужны только сетевые правила: комментарии, косметика (##, #@#, #?#, #$#, #%#)
                    // и скриптлеты в память не берём - это существенная часть EasyList.
                    if l.is_empty()
                        || l.starts_with('!')
                        || l.starts_with('[')
                        || l.contains("##")
                        || l.contains("#@#")
                        || l.contains("#?#")
                        || l.contains("#$#")
                        || l.contains("#%#")
                    {
                        continue;
                    }
                    lines.push(l.to_string());
                }
            }
        }
        let n = lines.len();
        let mut set = FilterSet::new(false);
        set.add_filters(&lines, ParseOptions::default());
        (Engine::from_filter_set(set, true), n)
    }

    fn skip_page(page: &str) -> bool {
        page.contains("youtube.com") || page.contains("youtube-nocookie.com")
    }

    pub fn install(
        webview: &wry::WebView,
        engine: Engine,
        page_url: Rc<RefCell<String>>,
        log_path: std::path::PathBuf,
    ) -> windows::core::Result<()> {
        unsafe {
            let controller = webview.controller();
            let core = controller.CoreWebView2()?;
            let env = core.cast::<ICoreWebView2_2>()?.Environment()?;
            // Подписываемся только на типы, где живёт реклама. Видео (MEDIA), шрифты, стили, документы
            // и WebSocket в обработчик не попадают совсем: WebView2 не прерывает ради них сетевой поток.
            for ctx in [
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER,
            ] {
                core.AddWebResourceRequestedFilter(&HSTRING::from("*"), ctx)?;
            }

            let mut logged = 0u32;
            let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let args = match args {
                    Some(a) => a,
                    None => return Ok(()),
                };
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                args.ResourceContext(&mut ctx)?;
                let request = args.Request()?;
                let mut uri = PWSTR::null();
                request.Uri(&mut uri)?;
                let url = take_pwstr(uri);
                if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ws://") || url.starts_with("wss://")) {
                    return Ok(());
                }
                if url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost") {
                    return Ok(());
                }
                let page = page_url.borrow().clone();
                if skip_page(&page) {
                    return Ok(());
                }
                let rtype = if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT {
                    "script"
                } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE {
                    "image"
                } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST {
                    "xmlhttprequest"
                } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH {
                    "fetch"
                } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING {
                    "ping"
                } else {
                    "other"
                };
                let (matched, bad) = match Request::new(&url, &page, rtype) {
                    Ok(req) => (engine.check_network_request(&req).matched, false),
                    Err(_) => (false, true),
                };
                // Диагностика: первые 400 проверенных запросов (тип, адрес, страница, результат)
                if DEBUG && logged < 400 {
                    logged += 1;
                    log(&log_path, &format!("{} {} page={} matched={} bad_request={}\n", rtype, url, page, matched, bad));
                }
                if matched {
                    if let Some(stream) = SHCreateMemStream(None) {
                        let resp = env.CreateWebResourceResponse(&stream, 403, &HSTRING::from("Blocked"), &HSTRING::from(""))?;
                        args.SetResponse(&resp)?;
                    }
                }
                Ok(())
            }));
            let mut token = EventRegistrationToken::default();
            core.add_WebResourceRequested(&handler, &mut token)?;
        }
        Ok(())
    }
}

// ====================== Камера и микрофон ======================
// MEDIA_MODE - кто получает камеру и микрофон:
//   0 - ничего не перехватываем: штатное окно WebView2 с вопросом (выбор можно запомнить для сайта);
//   1 - сразу разрешаем только сайтам из %LOCALAPPDATA%\Falco\trusted-media.txt и localhost / 127.0.0.1,
//       остальным остаётся штатный вопрос (рекомендуется);
//   2 - сразу разрешаем ЛЮБОМУ https-сайту (небезопасно: камеру получит любая страница и любой её iframe).
// Остальные разрешения (геолокация, уведомления и т.д.) не затрагиваются.
const MEDIA_MODE: u8 = 1;
const TRUSTED_MEDIA_EXAMPLE: &str = "# Сайты, которым разрешён доступ к камере и микрофону БЕЗ вопроса.\n# По одному домену в строке, поддомены включены. Например:\n# meet.google.com\n# Остальные сайты получат обычное окно с вопросом.\n";

fn load_trusted_media(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && valid_domain(l))
        .collect()
}

#[cfg(windows)]
mod media_permissions {
    use webview2_com::take_pwstr;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    };
    use webview2_com::PermissionRequestedEventHandler;
    use windows::core::PWSTR;
    use windows::Win32::System::WinRT::EventRegistrationToken;
    use wry::WebViewExtWindows;

    fn secure_origin(uri: &str) -> bool {
        if uri.starts_with("https://") {
            return true;
        }
        for base in ["http://localhost", "http://127.0.0.1"] {
            if let Some(rest) = uri.strip_prefix(base) {
                if rest.is_empty() || rest.starts_with(':') || rest.starts_with('/') {
                    return true;
                }
            }
        }
        false
    }

    // Uri - адрес того фрейма, который просит доступ (для iframe это адрес iframe, а не страницы)
    fn allowed(uri: &str, trusted: &[String]) -> bool {
        if !secure_origin(uri) {
            return false;
        }
        match super::MEDIA_MODE {
            2 => true,
            1 => match super::host_of(uri) {
                Some(h) => h == "localhost" || h == "127.0.0.1" || trusted.iter().any(|t| super::is_under(&h, t)),
                None => false,
            },
            _ => false,
        }
    }

    pub fn install(webview: &wry::WebView, trusted: Vec<String>) -> windows::core::Result<()> {
        unsafe {
            let controller = webview.controller();
            let core = controller.CoreWebView2()?;
            let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let args = match args {
                    Some(a) => a,
                    None => return Ok(()),
                };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
                args.PermissionKind(&mut kind)?;
                if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let url = take_pwstr(uri);
                    // Не разрешили - состояние не трогаем: WebView2 покажет обычное окно с вопросом
                    if allowed(&url, &trusted) {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    }
                }
                Ok(())
            }));
            let mut token = EventRegistrationToken::default();
            core.add_PermissionRequested(&handler, &mut token)?;
        }
        Ok(())
    }
}

fn js_str(s: &str) -> String {
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

// Скрипт перехода на адрес. save_first = true: перед уходом со страницы она сохраняет
// прокрутку и позицию видео (используется при смене вкладки, чтобы потом их восстановить).
fn nav_script(url: &str, save_first: bool) -> String {
    let save = if save_first { "if (window.__ulSaveForSwitch) window.__ulSaveForSwitch(); " } else { "" };
    format!("{}window.location.href = '{}';", save, js_str(url))
}

// ====================== Сохранение вкладок ======================
// %LOCALAPPDATA%\Falco\session.txt: 1-я строка - номер активной вкладки, далее по одному URL в строке.
const MAX_TABS: usize = 50;

// ====================== Защита ======================
// Лимит длины адреса и сообщения от страницы, число команд в секунду (защита от флуда)
const MAX_URL_LEN: usize = 4096;
const MAX_IPC_LEN: usize = 8192;
const MAX_IPC_PER_SEC: u32 = 60;
// Windows SmartScreen проверяет сайты и скачиваемые файлы на фишинг и вредоносное ПО.
// true = включён (рекомендуется), false = отключён (меньше памяти, но URL не уходят в Microsoft).
const SMARTSCREEN: bool = true;
// WebRTC (видеозвонки) не выдаёт сайтам локальные IP-адреса компьютера, только внешний.
// Если звонки в локальной сети перестанут соединяться - поставьте false.
const WEBRTC_HIDE_LOCAL_IP: bool = true;

// Секрет запуска: 192 бита случайности из системного ГСЧ (через RandomState), без лишних зависимостей.
// Его знает только наш скрипт в странице; Rust отбрасывает любое сообщение страницы без него.
fn make_secret() -> String {
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

// Хост из адреса (без пользователя, порта и точки в конце): "https://a@b.com:8080/x" -> "b.com"
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split(|c| c == '/' || c == '?' || c == '#').next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if let Some(v6) = auth.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        auth.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

// Подпись вкладки: в страницу уходят только имена хостов, а не полные адреса (в них бывают токены)
fn tab_label(url: &str) -> String {
    match host_of(url) {
        Some(h) => h.strip_prefix("www.").map(|x| x.to_string()).unwrap_or(h),
        None => "Новая вкладка".to_string(),
    }
}

fn tabs_labels(tabs: &[String]) -> String {
    tabs.iter().map(|u| tab_label(u)).collect::<Vec<_>>().join("|||")
}

// Куда разрешено переходить окну. Остальное (file:, ms-msdt:, search-ms:, javascript: и т.п.) отменяется.
fn nav_allowed(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "about" | "blob" | "chrome-error"
        ),
        None => false,
    }
}

// Не чаще одного запуска в 1,5 с: страница не сможет засыпать вас окнами
fn spawn_ok(last: &mut Option<std::time::Instant>) -> bool {
    let now = std::time::Instant::now();
    if let Some(t) = last {
        if now.duration_since(*t).as_millis() < 1500 {
            return false;
        }
    }
    *last = Some(now);
    true
}

// Замеры скорости загрузки страниц пишутся в %LOCALAPPDATA%\Falco\perf.log (для поиска причин медленной загрузки).
// Когда разберёмся, поставьте false.
const PERF_LOG: bool = false;

fn perf_log(dir: &Path, line: &str) {
    use std::io::Write;
    let path = dir.join("perf.log");
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 300_000 {
        return; // файл не растёт бесконечно
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
const DEFAULT_URL: &str = "https://duckduckgo.com";

fn is_http_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() > 8
        && s.len() <= MAX_URL_LEN
        && !s.bytes().any(|b| b <= b' ' || b == 0x7f)
        && !s.contains("|||") // разделитель списка вкладок
}

fn load_session(path: &Path) -> (Vec<String>, usize) {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut lines = text.lines();
    let active = lines.next().and_then(|l| l.trim().parse::<usize>().ok()).unwrap_or(0);
    let tabs: Vec<String> = lines
        .map(|l| l.trim())
        .filter(|l| is_http_url(l))
        .take(MAX_TABS)
        .map(|l| l.to_string())
        .collect();
    let active = if tabs.is_empty() { 0 } else { active.min(tabs.len() - 1) };
    (tabs, active)
}

// Пишет файл только если список реально изменился; через временный файл,
// чтобы обрыв записи не оставил битый session.txt.
fn save_session(path: &Path, last: &mut String, tabs: &[String], active: usize) {
    let mut s = active.to_string();
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

fn main() {
    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Falco")
        .build(&event_loop)
        .unwrap();

    // %LOCALAPPDATA%\Falco: сюда кладутся filters\*.txt и scripts\*.js
    // Запасные варианты - только личные папки пользователя (раньше был C:\ProgramData, куда пишут все пользователи ПК:
    // оттуда любой мог бы подложить скрипты и фильтры, которые выполняются на всех сайтах)
    let app_dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local")))
        .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())))
        .unwrap_or_else(std::env::temp_dir)
        .join("Falco");
    let filters_dir = app_dir.join("filters");
    let scripts_dir = app_dir.join("scripts");
    let _ = fs::create_dir_all(&filters_dir);
    let _ = fs::create_dir_all(&scripts_dir);
    let example = scripts_dir.join("_example.user.js");
    if !example.exists() {
        let _ = fs::write(&example, EXAMPLE_SCRIPT);
    }
    let trusted_media_path = app_dir.join("trusted-media.txt");
    if !trusted_media_path.exists() {
        let _ = fs::write(&trusted_media_path, TRUSTED_MEDIA_EXAMPLE);
    }
    // Секрет этого запуска: без него страница не может отправить Rust ни одной команды
    let ipc_secret = make_secret();
    let ipc_prefix = format!("{}:", ipc_secret);
    let filters = load_filters(&filters_dir);
    let user_scripts = load_user_scripts(&scripts_dir);
    let ad_hosts_js = filters.js.join(",");
    let cosmetic_js = cosmetic_json(&filters);
    // Правила DNS собираем сразу и освобождаем списки: дальше они в памяти не нужны
    let dns_rules = filters
        .dns
        .iter()
        .map(|h| format!("MAP {h} ~NOTFOUND, MAP *.{h} ~NOTFOUND"))
        .collect::<Vec<_>>()
        .join(", ");
    drop(filters);

    // Адрес из командной строки (--app=... или http...) = режим "приложения":
    // сессию не читаем и не перезаписываем.
    let cli_url = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--app=").map(|s| s.trim_matches('"').to_string()))
        .or_else(|| std::env::args().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://")))
        // Чужой аргумент вроде --app=file:///... или --app=javascript:... игнорируем
        .filter(|u| is_http_url(u));

    let session_path = app_dir.join("session.txt");
    let session_enabled = cli_url.is_none();
    let mut restored = false;
    let (tabs_init, current_init) = match cli_url {
        Some(u) => (vec![u], 0),
        None => {
            let (t, a) = load_session(&session_path);
            if t.is_empty() {
                (vec![DEFAULT_URL.to_string()], 0)
            } else {
                restored = true;
                (t, a)
            }
        }
    };
    let start_url = tabs_init[current_init].clone();

    // Окно, открытое кнопкой "в отдельное окно", получает --return=<id> и пишет свой список вкладок
    // в popout-<id>.txt. Когда оно закроется, окно-родитель заберёт эти вкладки к себе.
    let ret_id: Option<String> = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--return=").map(|s| s.to_string()))
        .filter(|s| !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_digit() || b == b'_'));
    // Куда это окно пишет список вкладок: основное - в session.txt, отдельное - в popout-<id>.txt
    let save_path: Option<PathBuf> = if session_enabled {
        Some(session_path)
    } else {
        ret_id.map(|id| app_dir.join(format!("popout-{}.txt", id)))
    };
    // Файлы popout-*, которые никто не забрал (родитель закрылся раньше), старше суток удаляем
    if session_enabled {
        if let Ok(rd) = fs::read_dir(&app_dir) {
            for e in rd.filter_map(|e| e.ok()) {
                if e.file_name().to_string_lossy().starts_with("popout-") {
                    let old = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .map_or(false, |d| d.as_secs() > 24 * 3600);
                    if old {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
        }
    }
    // Для JS: адреса восстановленных вкладок и id этого запуска (по нему отличаем записи прошлого запуска)
    let restore_urls_js = if restored { json_arr(&tabs_init) } else { "[]".to_string() };
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();

    let script = r#"
        (function() {
            try {
                const PERF = __PERF__;
                const NATIVE = __NATIVE__; // true: сборка с native-adblock (запросы проверяет нативный движок)

                // ---- Защищённый канал связи с Rust ----
                // Секрет запуска лежит только в замыкании этого скрипта, Rust отбрасывает любое сообщение без него.
                // Поэтому страница сайта (и любой её iframe) не может сама вызвать window.chrome.webview.postMessage('DETACH_TAB').
                // Нативную функцию захватываем сейчас, ДО кода сайта: подменить её позже страница уже не сможет.
                const IPC_T = '__IPC_TOKEN__';
                let rawPost = null;
                try {
                    if (window.chrome && window.chrome.webview && typeof window.chrome.webview.postMessage === 'function') {
                        rawPost = window.chrome.webview.postMessage.bind(window.chrome.webview);
                    } else if (window.ipc && typeof window.ipc.postMessage === 'function') {
                        rawPost = window.ipc.postMessage.bind(window.ipc);
                    }
                } catch (e) {}
                const ipcPost = (m) => { if (rawPost) { try { rawPost(IPC_T + ':' + m); } catch (e) {} } };
                // Наши глобальные функции, которые вызывает Rust: страница не может их подменить (и перехватить список вкладок).
                // Имена нарочно необычные, чтобы не столкнуться с глобальными именами сайтов
                const lockGlobal = (name, fn) => {
                    try { Object.defineProperty(window, name, { value: fn, writable: false, configurable: false, enumerable: false }); } catch (e) {}
                };
                const perfT0 = performance.now();
                if (window === window.top) ipcPost("SYNC_URL|" + window.location.href);

                // 1. БЛОКИРОВЩИК
                const adRegex = /(doubleclick\.net|google-analytics\.com|googlesyndication\.com|adservice\.google\.com|mc\.yandex\.ru|an\.yandex\.ru|amazon-adsystem\.com|appnexus\.com|criteo\.com|taboola\.com|yadro\.ru|top-fwz1\.mail\.ru|propellerads|popads|adk2x|betboom|1xbet|fonbet|melbet|awaps\.|a-ads\.com|mgid\.com|adskeeper|exoclick|clickadu|trafficstars|adfox|adriver|\/pagead\/|\/get_midroll_info)/i;
                const isYT = /(^|\.)youtube(-nocookie)?\.com$/i.test(window.location.hostname);
                // fetch() часто получает объект Request, а не строку - берём из него .url
                function urlOf(x) {
                    try { return typeof x === 'string' ? x : (x && x.url) ? x.url : String(x); } catch (e) { return ''; }
                }
                let AD_SET = null; // создаётся при первой проверке: во многих фреймах она не нужна
                function hostBlocked(h) {
                    if (!AD_SET) AD_SET = new Set('__AD_HOSTS__'.split(','));
                    while (h) {
                        if (AD_SET.has(h)) return true;
                        const i = h.indexOf('.');
                        if (i < 0) return false;
                        h = h.slice(i + 1);
                    }
                    return false;
                }
                function isBlocked(x) {
                    if (isYT) return false; // на YouTube сетевые запросы не трогаем: он реагирует на сбои буферизацией
                    const u = urlOf(x);
                    if (!u || u.startsWith('data:') || u.startsWith('blob:')) return false;
                    if (adRegex.test(u)) return true;
                    try { return hostBlocked(new URL(u, window.location.href).hostname.toLowerCase()); } catch (e) { return false; }
                }

                const originalFetch = window.fetch;
                window.fetch = async function(...args) {
                    if (isBlocked(args[0])) return new Response(null, { status: 204 });
                    return originalFetch.apply(this, args);
                };

                const originalXhrOpen = XMLHttpRequest.prototype.open;
                XMLHttpRequest.prototype.open = function(method, url, ...rest) {
                    if (isBlocked(url)) return originalXhrOpen.apply(this, [method, 'data:text/plain,', ...rest]);
                    return originalXhrOpen.apply(this, [method, url, ...rest]);
                };

                // ---- Новые окна сайта (target="_blank" и window.open) открываем новой вкладкой этого браузера ----
                // Сохраняем нативные функции ДО кода сайта, чтобы страница не могла подменить Date.now / performance.now
                const perfNow = performance.now.bind(performance);
                const ulLastOpen = { u: '', t: -1e9 };
                let lastTrustedGestureTime = -1e9;
                let lastGestureInteractive = false;

                // Проверяем, кликнул ли пользователь по интерактивному элементу (ссылке, кнопке), а не просто в фон страницы
                function isInteractiveTarget(el) {
                    if (!el || el.nodeType !== 1) return false;
                    if (el.closest && el.closest('a, area, button, input, select, textarea, summary, [role="button"], [role="link"], [tabindex]')) return true;
                    try { if (window.getComputedStyle(el).cursor === 'pointer') return true; } catch (e) {}
                    return false;
                }

                // Записываем только финальные события активации (без mousedown/mouseup, чтобы 1 клик не давал 3 попытки)
                const recordGesture = (e) => {
                    if (!e || !e.isTrusted) return;
                    if (e.type === 'keydown') {
                        if (e.key !== 'Enter' && e.key !== ' ') return;
                        lastGestureInteractive = isInteractiveTarget(e.target);
                    } else {
                        if (e.type === 'auxclick' && e.button !== 1) return;
                        if (e.type === 'click' && e.button !== 0) return;
                        lastGestureInteractive = isInteractiveTarget(e.target);
                    }
                    lastTrustedGestureTime = perfNow();
                };
                window.addEventListener('click', recordGesture, true);
                window.addEventListener('auxclick', recordGesture, true);
                window.addEventListener('keydown', recordGesture, true);

                function userGesture(e) {
                    if (e && e.isTrusted) return true;
                    return lastGestureInteractive && (perfNow() - lastTrustedGestureTime) < 150;
                }

                function openInTab(u, background) {
                    try {
                        const abs = new URL(u, window.location.href).href;
                        if (!/^https?:\/\//i.test(abs) || !rawPost) return false;
                        const now = perfNow();
                        // Сбрасываем жест и ограничиваем частоту: 1 клик = максимум 1 новая вкладка (не чаще раза в 500 мс)
                        lastTrustedGestureTime = -1e9;
                        lastGestureInteractive = false;
                        if (now - ulLastOpen.t < 500) return true;
                        ulLastOpen.u = abs; ulLastOpen.t = now;
                        ipcPost((background ? 'OPEN_TAB_BG|' : 'OPEN_TAB|') + abs);
                        return true;
                    } catch (e) { return false; }
                }

                window.open = function(u) {
                    if (u && userGesture(null)) openInTab(String(u), false);
                    return null;
                };

                // Патчим HTMLElement.prototype.click (также перехватывает HTMLAnchorElement и <area>)
                const origElemClick = HTMLElement.prototype.click;
                HTMLElement.prototype.click = function() {
                    const tag = this.tagName;
                    if (tag === 'A' || tag === 'AREA') {
                        const tgt = (this.target || '').toLowerCase();
                        if ((!this.isConnected || tgt === '_blank') && !this.hasAttribute('download') && !userGesture(null)) return;
                    }
                    return origElemClick.apply(this, arguments);
                };

                // Блокируем автоматическую отправку форм с target="_blank" без клика пользователя
                const origFormSubmit = HTMLFormElement.prototype.submit;
                HTMLFormElement.prototype.submit = function() {
                    if ((this.target || '').toLowerCase() === '_blank' && !userGesture(null)) return;
                    return origFormSubmit.apply(this, arguments);
                };
                document.addEventListener('submit', (e) => {
                    const f = e.target;
                    if (f && (f.target || '').toLowerCase() === '_blank' && !userGesture(e)) {
                        e.preventDefault();
                        e.stopImmediatePropagation();
                    }
                }, true);

                // Любая ссылка: колесо или Ctrl+клик -> фоновая вкладка; Shift+клик и target="_blank" -> новая вкладка
                function onLinkClick(e) {
                    if (e.defaultPrevented) return;
                    const mid = e.type === 'auxclick' && e.button === 1;
                    const left = e.type === 'click' && e.button === 0;
                    if (!mid && !left) return;
                    const a = e.target && e.target.closest ? e.target.closest('a[href], area[href]') : null;
                    if (!a || a.hasAttribute('download')) return;
                    const blank = (a.getAttribute('target') || '').toLowerCase() === '_blank';
                    let mode = null;
                    if (mid || (left && (e.ctrlKey || e.metaKey))) mode = 'bg';
                    else if (left && !e.altKey && (e.shiftKey || blank)) mode = 'fg';
                    if (!mode) return;
                    if (!userGesture(e)) {
                        // Гасим синтетические клики через dispatchEvent(new MouseEvent('click')), чтобы WebView2 не открыл NEWWIN
                        e.preventDefault();
                        e.stopImmediatePropagation();
                        return;
                    }
                    const href = typeof a.href === 'string' ? a.href : a.getAttribute('href');
                    if (openInTab(href, mode === 'bg')) e.preventDefault();
                }
                document.addEventListener('click', onLinkClick, true);
                document.addEventListener('auxclick', onLinkClick, true);

                // ---- YouTube: вырезаем рекламу из данных ДО того, как плеер и страница её увидят ----

                function setupYouTube() {
                    // По умолчанию выключено: YouTube грузит родной поток (VP9/AV1) без обходных путей.
                    // Включайте true только на ПК без аппаратного декодера VP9/AV1 (тогда видео тормозит на CPU).
                    const FORCE_H264 = false;
                    if (FORCE_H264) {
                        const badCodec = /av01|vp0?9/i;
                        if (window.MediaSource && MediaSource.isTypeSupported) {
                            const origSupported = MediaSource.isTypeSupported.bind(MediaSource);
                            MediaSource.isTypeSupported = function(type) { return badCodec.test(String(type)) ? false : origSupported(type); };
                        }
                        const origCanPlay = HTMLMediaElement.prototype.canPlayType;
                        HTMLMediaElement.prototype.canPlayType = function(type) { return badCodec.test(String(type)) ? '' : origCanPlay.apply(this, arguments); };
                        // YouTube спрашивает ещё и mediaCapabilities - без этой подмены он выберет VP9 и получит ошибку
                        if (navigator.mediaCapabilities && navigator.mediaCapabilities.decodingInfo) {
                            const origDI = navigator.mediaCapabilities.decodingInfo.bind(navigator.mediaCapabilities);
                            navigator.mediaCapabilities.decodingInfo = function(cfg) {
                                const ct = (cfg && cfg.video && cfg.video.contentType) || '';
                                if (badCodec.test(String(ct))) return Promise.resolve({ supported: false, smooth: false, powerEfficient: false });
                                return origDI(cfg);
                            };
                        }
                    }

                    const PLAYER_AD_KEYS = ['adPlacements', 'playerAds', 'adSlots', 'adBreakHeartbeatParams'];

                    // Быстрая точечная очистка ТОЛЬКО ответа плеера (выполняется за <0.1 мс вместо обхода 40 уровней вложенности)
                    function prunePlayerAds(data) {
                        if (!data || typeof data !== 'object') return data;
                        for (const k of PLAYER_AD_KEYS) {
                            if (k in data) delete data[k];
                        }
                        if (data.playerResponse && typeof data.playerResponse === 'object') {
                            for (const k of PLAYER_AD_KEYS) {
                                if (k in data.playerResponse) delete data.playerResponse[k];
                            }
                        }
                        return data;
                    }

                    // 1) Мгновенная очистка стартовых данных плеера (ytInitialData не трогаем — баннеры в ленте скроет CSS без тормозов)
                    let playerRespValue;
                    try {
                        Object.defineProperty(window, 'ytInitialPlayerResponse', {
                            configurable: true,
                            get() { return playerRespValue; },
                            set(v) { try { playerRespValue = prunePlayerAds(v); } catch (e) { playerRespValue = v; } }
                        });
                    } catch (e) {}

                    // 2) Перехват только JSON-ответов самого плеера (/youtubei/v1/player) без глобального хука на JSON.parse
                    const YT_PLAYER_API = /\/youtubei\/v1\/player/;
                    const origJson = Response.prototype.json;
                    Response.prototype.json = function() {
                        const p = origJson.apply(this, arguments);
                        if (!YT_PLAYER_API.test(this.url || '')) return p;
                        return p.then((d) => { try { return prunePlayerAds(d); } catch (e) { return d; } });
                    };

                    // 3) Запасной турбо-скипер (если реклама вшита в поток).
                    // Постоянного опроса раз в 200 мс больше нет: за классом плеера (ad-showing) следит MutationObserver,
                    // а быстрый цикл 200 мс включается только на время самой рекламы и выключается после неё.
                    let inAd = false, saved = null, adTimer = null, watchedPlayer = null, playerObs = null;
                    const isAdClass = (pl) => pl.classList.contains('ad-showing') || pl.classList.contains('ad-interrupting');
                    function adStep() {
                        const player = document.getElementById('movie_player');
                        if (!player) return;
                        const video = player.querySelector('video');

                        const skip = player.querySelector('.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button, .ytp-ad-skip-button-container button');
                        if (skip) skip.click();

                        if (isAdClass(player) && video) {
                            if (!inAd) { inAd = true; saved = { rate: video.playbackRate, muted: video.muted }; }
                            video.muted = true;
                            video.playbackRate = 16;
                            if (isFinite(video.duration) && video.duration > 0) {
                                video.currentTime = video.duration;
                            }
                            if (!adTimer) adTimer = setInterval(adStep, 200);
                        } else {
                            if (inAd) {
                                inAd = false;
                                if (video && saved) { video.playbackRate = saved.rate; video.muted = saved.muted; }
                                saved = null;
                            }
                            if (adTimer) { clearInterval(adTimer); adTimer = null; }
                        }
                    }
                    function removeWall() {
                        const wall = document.querySelector('ytd-enforcement-message-view-model');
                        if (!wall) return;
                        const dlg = wall.closest('tp-yt-paper-dialog') || wall;
                        dlg.remove();
                        const backdrop = document.querySelector('tp-yt-iron-overlay-backdrop');
                        if (backdrop) backdrop.remove();
                        const v = document.querySelector('#movie_player video');
                        if (v && v.paused) { try { v.play().catch(() => {}); } catch (e) {} }
                    }
                    // Раз в секунду и только на видимой вкладке: подключить наблюдатель к плееру (YouTube мог его пересоздать)
                    // и убрать окно-предупреждение о блокировщике.
                    function ytWatch() {
                        if (document.hidden) return;
                        const player = document.getElementById('movie_player');
                        if (player && player !== watchedPlayer) {
                            watchedPlayer = player;
                            if (playerObs) playerObs.disconnect();
                            // Только атрибут class самого плеера (без subtree): на обычные смены класса реагируем одной проверкой
                            playerObs = new MutationObserver(() => { if (inAd || isAdClass(player)) adStep(); });
                            playerObs.observe(player, { attributes: true, attributeFilter: ['class'] });
                            adStep();
                        }
                        removeWall();
                    }
                    setInterval(ytWatch, 1000);
                    window.addEventListener('yt-navigate-finish', ytWatch);
                }
                if (isYT) setupYouTube();

                // ---- Блокировка рекламы на уровне DOM (script / iframe / img), кроме YouTube ----
                if (!isYT) {
                    const patchSrc = (proto, prop) => {
                        const d = Object.getOwnPropertyDescriptor(proto, prop);
                        if (!d || !d.set) return;
                        Object.defineProperty(proto, prop, {
                            configurable: true, enumerable: d.enumerable, get: d.get,
                            set(v) { if (isBlocked(v)) return; d.set.call(this, v); }
                        });
                    };
                    patchSrc(HTMLScriptElement.prototype, 'src');
                    patchSrc(HTMLIFrameElement.prototype, 'src');
                    patchSrc(HTMLImageElement.prototype, 'src');
                    patchSrc(HTMLEmbedElement.prototype, 'src');

                    const SRC_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED', 'OBJECT', 'LINK', 'SOURCE']);
                    const origSetAttr = Element.prototype.setAttribute;
                    Element.prototype.setAttribute = function(name, value) {
                        // Сначала самые дешёвые проверки (длина имени 3-4 символа и нужный тег): подавляющее
                        // большинство вызовов setAttribute на странице проходит без try / toLowerCase / isBlocked
                        if (typeof name === 'string' && (name.length === 3 || name.length === 4) && SRC_TAGS.has(this.tagName)) {
                            try {
                                const n = name.toLowerCase();
                                if ((n === 'src' || n === 'href' || n === 'data') && isBlocked(value)) return;
                            } catch (e) {}
                        }
                        return origSetAttr.apply(this, arguments);
                    };

                    if (navigator.sendBeacon) {
                        const origBeacon = navigator.sendBeacon.bind(navigator);
                        navigator.sendBeacon = function(url, data) { return isBlocked(url) ? true : origBeacon(url, data); };
                    }

                    // Невидимая ссылка target=_blank, которую скрипт "кликает" сам - классика для pop-under рекламы
                    const origAClick = HTMLAnchorElement.prototype.click;
                    HTMLAnchorElement.prototype.click = function() {
                        if (!this.isConnected && this.target === '_blank' && !this.hasAttribute('download')) return;
                        return origAClick.apply(this, arguments);
                    };

                    // Теги, вставленные парсером страницы (не через JS), ловим постфактум
                    const AD_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED']);
                    const scanNode = (n) => {
                        if (!n || n.nodeType !== 1) return;
                        if (AD_TAGS.has(n.tagName) && isBlocked(n.src)) { n.remove(); return; }
                        if (n.firstElementChild) {
                            n.querySelectorAll('script[src],iframe[src],img[src],embed[src]').forEach((el) => { if (isBlocked(el.src)) el.remove(); });
                        }
                    };
                    // При native-adblock в верхнем окне наблюдатель за всем DOM не нужен (запросы режет нативный движок),
                    // в iframe он остаётся: туда нативная проверка может не дотянуться.
                    if (!(NATIVE && window === window.top)) {
                        new MutationObserver((muts) => {
                            for (const m of muts) for (const n of m.addedNodes) scanNode(n);
                        }).observe(document, { childList: true, subtree: true });
                    }
                }

                const css = `
                    .b-adv, .b-banner, #branding, .branding, .ad-container, 
                    [id*="yandex_rtb"], [class*="yandex-rtb"],
                    [id^="ad_"], [class^="ad_"], [class*="-ad-"],
                    div[data-ad], div[data-banner],
                    ins.adsbygoogle, [id*="adfox"], [class*="adfox"], [id^="div-gpt-ad"], [id^="google_ads"],
                    iframe[src*="adfox"], iframe[src*="adriver"],
                    a[href*="1xbet"], a[href*="fonbet"], a[href*="casino"] { 
                        display: none !important; opacity: 0 !important; pointer-events: none !important; width: 0 !important; height: 0 !important;
                    }
                `;
                // Запасной CSS для YouTube: прячет то, что успело отрисоваться
                const ytCss = `
                    ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-display-ad-renderer,
                    ytd-promoted-sparkles-web-renderer, ytd-promoted-video-renderer, ytd-compact-promoted-video-renderer,
                    ytd-banner-promo-renderer, ytd-statement-banner-renderer, ytd-primetime-promo-renderer,
                    ytd-action-companion-ad-renderer, ytd-player-legacy-desktop-watch-ads-renderer,
                    ytd-brand-video-singleton-renderer, ytd-brand-video-shelf-renderer,
                    #masthead-ad, #player-ads, .ytp-ad-overlay-container { display: none !important; }
                    ytd-rich-item-renderer:has(ytd-ad-slot-renderer),
                    ytd-rich-item-renderer:has(ytd-in-feed-ad-layout-renderer) { display: none !important; }
                `;
                function injectCss() {
                    if (!document.head) { requestAnimationFrame(injectCss); return; }
                    if (!isYT && !document.getElementById('ultralight-css')) {
                        const style = document.createElement('style'); style.id = 'ultralight-css'; style.textContent = css; document.head.appendChild(style);
                    }
                    if (isYT && !document.getElementById('ultralight-yt-css')) {
                        const style = document.createElement('style'); style.id = 'ultralight-yt-css'; style.textContent = ytCss; document.head.appendChild(style);
                    }
                }
                injectCss();

                // Косметические фильтры из filters/*.txt (только верхний фрейм, не YouTube).
                // Каждый селектор проверяется отдельно: один битый не ломает остальные.
                // Список косметических правил хранится строкой и разбирается один раз, только в главном окне страницы
                let COSM = null;
                let cosmCss = null, cosmEl = null;
                function applyCosmetic() {
                    if (isYT || window !== window.top) return;
                    if (cosmCss === null) {
                        // Данные косметики лежат в отдельном скрипте, который выполняется только в верхнем окне
                        // (в iframe огромная строка не создаётся). Если он ещё не выполнился - попробуем позже.
                        if (typeof window.__ulCosmRaw !== 'function') return;
                        let raw = '{"g":[],"h":{}}';
                        try { raw = window.__ulCosmRaw(); } catch (e) {}
                        try { delete window.__ulCosmRaw; } catch (e) {}
                        try { COSM = JSON.parse(raw); } catch (e) { COSM = { g: [], h: {} }; }
                        const sels = COSM.g.slice();
                        let h = window.location.hostname.toLowerCase();
                        while (h) {
                            if (COSM.h[h]) for (let i = 0; i < COSM.h[h].length; i++) sels.push(COSM.h[h][i]);
                            const i = h.indexOf('.');
                            if (i < 0) break;
                            h = h.slice(i + 1);
                        }
                        const ok = [];
                        for (let i = 0; i < sels.length; i++) {
                            try { if (!window.CSS || CSS.supports('selector(' + sels[i] + ')')) ok.push(sels[i]); } catch (e) {}
                        }
                        const parts = [];
                        for (let i = 0; i < ok.length; i += 100) parts.push(ok.slice(i, i + 100).join(',') + '{display:none!important}');
                        cosmCss = parts.join('\n');
                    }
                    if (!cosmCss) return;
                    const root = document.head || document.documentElement;
                    if (!root) { requestAnimationFrame(applyCosmetic); return; }
                    if (!cosmEl || !cosmEl.isConnected) {
                        cosmEl = document.createElement('style');
                        cosmEl.textContent = cosmCss;
                        root.appendChild(cosmEl);
                    }
                }
                applyCosmetic();

                let ui = null; // { container, trigger, tabsRow, input, show, hide }

                // ---- URL-синхронизация ----
                let lastSyncedUrl = window.location.href;

                function syncUrl() {
                    if (window !== window.top) return; // iframes не должны портить список вкладок
                    const href = window.location.href;
                    if (href !== lastSyncedUrl) {
                        lastSyncedUrl = href;
                        ipcPost('SYNC_URL|' + href);
                    }
                    if (ui && document.activeElement !== ui.input) ui.input.value = href;
                }
                const onNavigation = syncUrl;

                // Сначала выполняем оригинальный pushState/replaceState, и только потом читаем URL
                const origPush = history.pushState;
                history.pushState = function() { const r = origPush.apply(this, arguments); onNavigation(); return r; };
                const origReplace = history.replaceState;
                history.replaceState = function() { const r = origReplace.apply(this, arguments); onNavigation(); return r; };
                window.addEventListener('popstate', onNavigation);
                window.addEventListener('hashchange', onNavigation);
                window.addEventListener('yt-navigate-finish', onNavigation);

                // 2. ИНТЕРФЕЙС (создаётся один раз, при удалении сайтом просто возвращается в DOM)
                if (window !== window.top) return;

                let tabsState = { list: [], active: 0 };

                function drawTabs() {
                    if (!ui) return;
                    const row = ui.tabsRow;
                    row.replaceChildren(); // НЕ innerHTML: YouTube включает Trusted Types, и innerHTML = '' бросает ошибку

                    tabsState.list.forEach((url, i) => {
                        let tabEl = document.createElement('div');
                        let isActive = (i === tabsState.active);

                        tabEl.style.cssText = `
                            min-width: 120px; max-width: 200px; height: 28px; margin-top: 2px; margin-right: 2px;
                            background: ${isActive ? '#303134' : '#171717'}; color: ${isActive ? '#fff' : '#9aa0a6'};
                            border-radius: 8px 8px 0 0; display: flex; align-items: center; justify-content: space-between;
                            padding: 0 10px; font-size: 13px; cursor: pointer; user-select: none; transition: background 0.2s;
                        `;
                        tabEl.onmouseover = () => { if(!isActive) tabEl.style.background = '#202124'; };
                        tabEl.onmouseout = () => { if(!isActive) tabEl.style.background = '#171717'; };
                        tabEl.onclick = (e) => { if (e.isTrusted) ipcPost('SWITCH_TAB|' + i); };

                        let titleSpan = document.createElement('span');
                        titleSpan.textContent = url || 'Новая вкладка'; // Rust присылает только имя хоста, не полный адрес
                        titleSpan.style.cssText = `overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-grow: 1; margin-right: 8px;`;

                        let closeBtn = document.createElement('span'); closeBtn.textContent = '×';
                        closeBtn.style.cssText = `font-size: 16px; width: 18px; height: 18px; display: flex; align-items: center; justify-content: center; border-radius: 50%; font-weight: bold; cursor: pointer;`;
                        closeBtn.onmouseover = () => closeBtn.style.background = '#5f6368';
                        closeBtn.onmouseout = () => closeBtn.style.background = 'transparent';
                        closeBtn.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) ipcPost('CLOSE_TAB|' + i); };

                        tabEl.appendChild(titleSpan); tabEl.appendChild(closeBtn); row.appendChild(tabEl);
                    });

                    let newTabBtn = document.createElement('div'); newTabBtn.textContent = '+';
                    newTabBtn.style.cssText = `min-width: 28px; height: 28px; margin-top: 2px; color: #9aa0a6; font-size: 20px; display: flex; align-items: center; justify-content: center; cursor: pointer; border-radius: 50%;`;
                    newTabBtn.onmouseover = () => newTabBtn.style.background = '#303134';
                    newTabBtn.onmouseout = () => newTabBtn.style.background = 'transparent';
                    newTabBtn.onclick = (e) => { if (e.isTrusted) ipcPost('new_tab'); };
                    row.appendChild(newTabBtn);
                }

                // Определяем сразу: ответ от Rust может прийти раньше, чем построен интерфейс
                lockGlobal('__ulRT', function(tabsString, activeIndex) {
                    if (typeof tabsString !== 'string') return;
                    tabsState = { list: tabsString.split('|||'), active: activeIndex | 0 };
                    drawTabs();
                });

                function buildUI() {
                    const container = document.createElement('div'); container.id = 'light-browser-ui';
                    container.style.cssText = `position: fixed; top: -75px; left: 0; width: 100%; height: 75px; background: #202124; z-index: 2147483647; font-family: sans-serif; box-shadow: 0 4px 10px rgba(0,0,0,0.5); display: flex; flex-direction: column; transition: top 0.3s cubic-bezier(0.25, 0.8, 0.25, 1);`;

                    const tabsRow = document.createElement('div'); tabsRow.id = 'light-tabs-row';
                    tabsRow.style.cssText = `display: flex; height: 32px; background: #000; overflow-x: auto; padding: 0 5px;`;

                    const urlRow = document.createElement('div');
                    urlRow.style.cssText = `display: flex; height: 43px; align-items: center; padding: 0 10px; box-sizing: border-box;`;

                    const trigger = document.createElement('div'); trigger.id = 'light-trigger-zone';
                    trigger.style.cssText = `position: fixed; top: 0; left: 0; width: 100%; height: 15px; z-index: 2147483646; background: transparent;`;

                    const input = document.createElement('input'); input.type = 'text'; input.value = window.location.href;
                    input.style.cssText = `flex: 1; min-width: 0; height: 30px; border-radius: 15px; border: none; padding: 0 20px; background: #303134; color: #fff; outline: none; font-size: 14px;`;

                    input.addEventListener('keydown', (e) => {
                        if (e.key === 'Enter') {
                            let query = input.value.trim();
                            if (query.includes('.') && !query.includes(' ')) {
                                if (!query.startsWith('http')) query = 'https://' + query;
                                window.location.href = query;
                            } else { window.location.href = 'https://duckduckgo.com/?q=' + encodeURIComponent(query); }
                        }
                    });
                    input.addEventListener('focus', () => input.select());

                    const show = () => {
                        container.style.top = '0';
                        input.value = window.location.href;
                        ipcPost('GET_TABS');
                    };
                    const hide = () => { if (document.activeElement !== input) container.style.top = '-75px'; };

                    trigger.addEventListener('mouseenter', show);
                    container.addEventListener('mouseleave', hide);

                    // Кнопки навигации: назад / вперёд / обновить
                    function navButton(symbol, title, onClick) {
                        const b = document.createElement('div');
                        b.textContent = symbol; b.title = title;
                        b.style.cssText = `width: 30px; height: 30px; margin-right: 4px; flex: none; border-radius: 50%; color: #9aa0a6; font-size: 18px; display: flex; align-items: center; justify-content: center; cursor: pointer; user-select: none;`;
                        b.onmouseover = () => { b.style.background = '#303134'; b.style.color = '#fff'; };
                        b.onmouseout = () => { b.style.background = 'transparent'; b.style.color = '#9aa0a6'; };
                        b.onclick = onClick;
                        return b;
                    }
                    urlRow.appendChild(navButton('\u2190', 'Назад (Alt+←)', () => history.back()));
                    urlRow.appendChild(navButton('\u2192', 'Вперёд (Alt+→)', () => history.forward()));
                    urlRow.appendChild(navButton('\u21BB', 'Обновить (F5)', () => location.reload()));

                    // Правый верхний угол: вывести текущую вкладку в отдельное окно (для видеозвонков)
                    const popBtn = navButton('\u2197', 'Вывести текущую вкладку в отдельное окно (для видеозвонков)', (e) => { if (e && e.isTrusted) ipcPost('DETACH_TAB'); });
                    popBtn.style.marginLeft = '6px'; popBtn.style.marginRight = '0';

                    container.appendChild(tabsRow); urlRow.appendChild(input); urlRow.appendChild(popBtn); container.appendChild(urlRow);

                    ui = { container, trigger, tabsRow, input, show, hide };
                    // На 2 секунды показывает полосу вкладок (Rust вызывает, когда вкладка открыта в фоне)
                    lockGlobal('__ulFlash', function() {
                        if (!ui) return;
                        ui.container.style.top = '0';
                        setTimeout(() => { if (ui && !ui.container.matches(':hover')) ui.hide(); }, 2000);
                    });
                    drawTabs();
                    ipcPost('GET_TABS');
                }

                // ---- Ручное скрытие элементов: Ctrl+Shift+X (выбрать), Ctrl+Shift+Z (отменить последнее) ----
                // Правила хранятся в localStorage и действуют только на этом сайте (адресе).
                const RULES_KEY = 'ultralight_hidden';
                let userRules = [];
                try { userRules = JSON.parse(localStorage.getItem(RULES_KEY) || '[]'); } catch (e) { userRules = []; }
                if (!Array.isArray(userRules)) userRules = [];
                function saveRules() { try { localStorage.setItem(RULES_KEY, JSON.stringify(userRules)); } catch (e) {} }
                function ensureUserCss() {
                    if (!document.head) return;
                    let st = document.getElementById('ultralight-user-css');
                    if (!st) { st = document.createElement('style'); st.id = 'ultralight-user-css'; document.head.appendChild(st); }
                    const rules = userRules.map((s) => s + ' { display: none !important; }').join('\n');
                    if (st.textContent !== rules) st.textContent = rules;
                }
                (function applyWhenReady() {
                    if (!document.head) { requestAnimationFrame(applyWhenReady); return; }
                    ensureUserCss();
                })();

                function toast(msg) {
                    if (!document.documentElement) return;
                    const t = document.createElement('div');
                    t.textContent = msg;
                    t.style.cssText = 'position:fixed;left:16px;bottom:16px;max-width:60%;padding:10px 14px;background:#202124;color:#fff;font:13px sans-serif;border-radius:8px;z-index:2147483647;box-shadow:0 2px 10px rgba(0,0,0,.5);pointer-events:none;';
                    document.documentElement.appendChild(t);
                    setTimeout(() => t.remove(), 3500);
                }

                // Имена вроде b46fec00c0 или b-post__g0d7f9017fbdajr53w82a сайт генерирует случайно (чтобы обходить блокировщики):
                // на них нельзя опираться в сохраняемом правиле, после перезагрузки они уже другие
                const looksRandom = (s) => s.length >= 8 && /\d/.test(s) && /[A-Za-z]/.test(s);
                function cssPath(el) {
                    const parts = [];
                    while (el && el.nodeType === 1 && el !== document.documentElement && parts.length < 5) {
                        if (el.id && /^[A-Za-z][\w-]{2,}$/.test(el.id) && !looksRandom(el.id)
                            && document.querySelectorAll('#' + CSS.escape(el.id)).length === 1) {
                            parts.unshift('#' + CSS.escape(el.id));
                            break;
                        }
                        let part = el.tagName.toLowerCase();
                        const cls = Array.from(el.classList).filter((c) => /^[A-Za-z_-][\w-]{2,}$/.test(c) && !looksRandom(c)).slice(0, 2);
                        if (cls.length) part += '.' + cls.map((c) => CSS.escape(c)).join('.');
                        const parent = el.parentElement;
                        if (parent) {
                            const same = Array.from(parent.children).filter((c) => c.tagName === el.tagName);
                            if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
                        }
                        parts.unshift(part);
                        el = el.parentElement;
                    }
                    return parts.join(' > ');
                }

                let picking = false, pickEl = null, hl = null;
                const isOurs = (el) => !el || el.id === 'light-browser-ui' || el.id === 'light-trigger-zone' || (ui && ui.container.contains(el));
                function highlight(el) {
                    pickEl = el;
                    if (!hl) {
                        hl = document.createElement('div');
                        hl.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483645;outline:2px solid #ff4d4f;background:rgba(255,77,79,.18);';
                        document.documentElement.appendChild(hl);
                    }
                    const r = el.getBoundingClientRect();
                    hl.style.left = r.left + 'px'; hl.style.top = r.top + 'px';
                    hl.style.width = r.width + 'px'; hl.style.height = r.height + 'px';
                }
                const swallow = (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); };
                // События мыши внутри iframe не доходят до родительской страницы, поэтому на время выбора
                // над всей страницей лежит прозрачный слой. Чтобы узнать, что под ним, слой на миг делаем "прозрачным для мыши".
                let pickLayer = null;
                const onPickMove = (e) => {
                    if (pickLayer) pickLayer.style.pointerEvents = 'none';
                    const el = document.elementFromPoint(e.clientX, e.clientY);
                    if (pickLayer) pickLayer.style.pointerEvents = 'auto';
                    if (el && !isOurs(el)) highlight(el);
                };
                const onPickClick = (e) => {
                    swallow(e);
                    const el = pickEl;
                    stopPick();
                    if (!el) return;
                    if (el === document.body || el === document.documentElement) { toast('Нельзя скрыть всю страницу'); return; }
                    const sel = cssPath(el);
                    if (!sel) return;
                    userRules.push(sel); saveRules(); ensureUserCss();
                    try { console.log('[Falco] скрыт элемент:', sel, String(el.outerHTML).slice(0, 400)); } catch (e2) {}
                    toast('Скрыто <' + el.tagName.toLowerCase() + '>: ' + sel + '   (Ctrl+Shift+Z - отменить)');
                };
                const onPickKey = (e) => {
                    if (e.key === 'Escape') { swallow(e); stopPick(); toast('Отменено'); }
                    else if (e.key === 'ArrowUp' && pickEl && pickEl.parentElement && pickEl.parentElement !== document.documentElement) {
                        swallow(e); highlight(pickEl.parentElement);
                    }
                };
                const PICK_BLOCKED = ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'auxclick', 'contextmenu'];
                function startPick() {
                    if (picking) return;
                    picking = true; pickEl = null;
                    pickLayer = document.createElement('div');
                    pickLayer.style.cssText = 'position:fixed;left:0;top:0;width:100%;height:100%;z-index:2147483644;cursor:crosshair;background:transparent;pointer-events:auto;';
                    document.documentElement.appendChild(pickLayer);
                    // Если фокус остался внутри iframe, клавиши (Esc, стрелка вверх) не дойдут до страницы
                    try { if (document.activeElement && document.activeElement.blur) document.activeElement.blur(); window.focus(); } catch (e) {}
                    window.addEventListener('mousemove', onPickMove, true);
                    window.addEventListener('click', onPickClick, true);
                    PICK_BLOCKED.forEach((t) => window.addEventListener(t, swallow, true));
                    window.addEventListener('keydown', onPickKey, true);
                    toast('Кликните по элементу, который нужно скрыть. Стрелка вверх - выбрать родителя, Esc - отмена');
                }
                function stopPick() {
                    picking = false; pickEl = null;
                    window.removeEventListener('mousemove', onPickMove, true);
                    if (pickLayer) { pickLayer.remove(); pickLayer = null; }
                    window.removeEventListener('click', onPickClick, true);
                    PICK_BLOCKED.forEach((t) => window.removeEventListener(t, swallow, true));
                    window.removeEventListener('keydown', onPickKey, true);
                    if (hl) { hl.remove(); hl = null; }
                }
                function undoRule() {
                    const s = userRules.pop();
                    saveRules(); ensureUserCss();
                    toast(s ? 'Правило удалено: ' + s : 'Для этого сайта нет сохранённых правил');
                }

                // ---- Сохранение прокрутки, позиции видео и текста в полях ввода ----
                // Состояние лежит в localStorage сайта, по ключу страницы. Восстанавливается ОДИН раз:
                //  1) при возврате на вкладку: перед сменой Rust вызывает window.__ulSaveForSwitch();
                //  2) после перезапуска браузера: для страниц, которые были открыты вкладками (session.txt).
                // При обычных переходах (ссылка, ввод адреса, F5) ничего не восстанавливается.
                const STATE_KEY = 'ultralight_tabstate';
                const STATE_TTL = 12 * 60 * 60 * 1000;           // возврат на вкладку
                const RESTART_TTL = 7 * 24 * 60 * 60 * 1000;     // после перезапуска браузера
                const STATE_MAX = 60;
                const RUN_ID = '__RUN_ID__';                      // id запуска (подставляет Rust)
                const RESTORE_URLS = __RESTORE_URLS__;            // вкладки из session.txt ([] - сессии нет)
                const FORM_MAX = 20, FORM_LEN = 2000;
                let scrollDone = true, videoDone = true, formDone = true; // false, пока идёт восстановление
                let switching = false;
                let scrollTimer = null;

                function stateKey(h) {
                    try {
                        const u = new URL(h || window.location.href);
                        if (/(^|\.)youtube\.com$/i.test(u.hostname) && u.pathname === '/watch') return u.origin + '/watch?v=' + (u.searchParams.get('v') || '');
                        return u.origin + u.pathname + u.search; // без #фрагмента
                    } catch (e) { return h || window.location.href; }
                }
                const RESTORE_KEYS = new Set(RESTORE_URLS.map((u) => stateKey(u)));
                function readStates() {
                    try {
                        const o = JSON.parse(localStorage.getItem(STATE_KEY) || '{}');
                        return (o && typeof o === 'object' && !Array.isArray(o)) ? o : {};
                    } catch (e) { return {}; }
                }
                function writeStates(o) {
                    try {
                        // Записи старше недели удаляем: сохранённый текст не должен лежать в профиле вечно
                        const nowTs = Date.now();
                        Object.keys(o).forEach((k) => { if (!o[k] || nowTs - (o[k].ts || 0) > RESTART_TTL) delete o[k]; });
                        const keys = Object.keys(o);
                        if (keys.length > STATE_MAX) {
                            keys.sort((a, b) => (o[a].ts || 0) - (o[b].ts || 0));
                            keys.slice(0, keys.length - STATE_MAX).forEach((k) => { delete o[k]; });
                        }
                        localStorage.setItem(STATE_KEY, JSON.stringify(o));
                    } catch (e) {}
                }
                const adActive = () => !!document.querySelector('#movie_player.ad-showing, #movie_player.ad-interrupting');
                function mainVideo() {
                    const vids = document.querySelectorAll('video');
                    for (let i = 0; i < vids.length; i++) {
                        const v = vids[i];
                        if (isFinite(v.duration) && v.duration > 60 && v.readyState > 0 && !adActive()) return v;
                    }
                    return null;
                }

                // ---- Текстовые поля (поиск, комментарии, формы). Пароли и платёжные поля не сохраняются ----
                function fieldId(el) {
                    if (el.id) return '#' + el.id;
                    if (el.name) return '@' + el.name;
                    return null;
                }
                function collectForm() {
                    const f = {};
                    let n = 0;
                    const els = document.querySelectorAll('textarea, input[type=text], input[type=search], input:not([type])');
                    for (let i = 0; i < els.length && n < FORM_MAX; i++) {
                        const el = els[i];
                        if (ui && ui.container.contains(el)) continue; // адресная строка самого браузера
                        const id = fieldId(el);
                        if (!id || !el.value || el.value.length > FORM_LEN) continue;
                        const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
                        if (/password|cc-|one-time|username|email|tel|address|postal|bday/.test(ac)) continue;
                        if (/pass|card|cvv|cvc|otp|secret|token|login|e-?mail|phone|addr|iban|ssn|passport|birth|account|\bpin\b|\bsms\b/i.test((el.id || '') + ' ' + (el.name || ''))) continue;
                        f[id] = el.value; n++;
                    }
                    return n ? f : null;
                }
                function startFormRestore(f) {
                    const pending = Object.assign({}, f);
                    let ticks = 0, timer = null;
                    formDone = false;
                    // Если пользователь начал печатать сам, его ввод не перезаписываем
                    const onUser = (e) => {
                        if (!e.isTrusted) return;
                        const id = e.target && e.target.nodeType === 1 ? fieldId(e.target) : null;
                        if (id) delete pending[id];
                    };
                    const stop = () => {
                        clearInterval(timer);
                        document.removeEventListener('input', onUser, true);
                        formDone = true;
                    };
                    document.addEventListener('input', onUser, true);
                    timer = setInterval(() => {
                        ticks++;
                        Object.keys(pending).forEach((id) => {
                            const el = id.charAt(0) === '#' ? document.getElementById(id.slice(1)) : document.getElementsByName(id.slice(1))[0];
                            if (!el) return; // поле ещё не появилось (SPA) - ждём
                            const isTa = el instanceof HTMLTextAreaElement;
                            if (isTa || el instanceof HTMLInputElement) {
                                if (!el.value) {
                                    const d = Object.getOwnPropertyDescriptor(isTa ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, 'value');
                                    if (d && d.set) d.set.call(el, String(pending[id])); else el.value = String(pending[id]);
                                    el.dispatchEvent(new Event('input', { bubbles: true }));
                                    el.dispatchEvent(new Event('change', { bubbles: true }));
                                }
                            }
                            delete pending[id];
                        });
                        if (!Object.keys(pending).length || ticks >= 40) stop(); // ~10 секунд
                    }, 250);
                }

                function saveState() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const key = stateKey();
                    const states = readStates();
                    const old = states[key];
                    let x = Math.round(window.scrollX), y = Math.round(window.scrollY), t = 0;
                    let f = collectForm();
                    const v = mainVideo();
                    if (v && !window.location.pathname.startsWith('/shorts') && v.currentTime > 5 && v.currentTime < v.duration - 10) {
                        t = Math.floor(v.currentTime);
                    }
                    // Пока восстановление не закончено, не затираем ещё не применённые сохранённые значения
                    if (old) {
                        if (!scrollDone) { x = old.x || 0; y = old.y || 0; }
                        if (!videoDone) { t = old.t || 0; }
                        if (!formDone) { f = old.f || null; }
                    }
                    if (!x && !y && !t && !f) delete states[key];
                    else states[key] = { x: x, y: y, t: t, f: f || undefined, sw: switching, run: RUN_ID, ts: Date.now() };
                    writeStates(states);
                }

                function startScrollRestore(x, y) {
                    let ticks = 0, stable = 0, stopped = false, timer = null;
                    const INPUT_EVENTS = ['wheel', 'touchstart', 'mousedown', 'keydown'];
                    const stop = () => {
                        if (stopped) return;
                        stopped = true; scrollDone = true;
                        clearInterval(timer);
                        INPUT_EVENTS.forEach((ev) => window.removeEventListener(ev, stop, true));
                    };
                    // Любое действие пользователя отменяет автоматическую прокрутку
                    INPUT_EVENTS.forEach((ev) => window.addEventListener(ev, stop, { capture: true, passive: true }));
                    timer = setInterval(() => {
                        ticks++;
                        const root = document.documentElement;
                        const h = Math.max(root ? root.scrollHeight : 0, document.body ? document.body.scrollHeight : 0);
                        const maxY = h - window.innerHeight;
                        if (maxY >= y - 2) {
                            window.scrollTo(x, y);
                            if (Math.abs(window.scrollY - y) <= 3) stable++; else stable = 0;
                            if (stable >= 3) stop();
                        } else if (maxY > 0 && ticks <= 12) {
                            window.scrollTo(x, maxY); // страница ещё короткая: тянем вниз ~2 секунды, чтобы подгрузились ленты
                        }
                        if (ticks >= 30) stop(); // ~4.5 секунды
                    }, 150);
                }

                function startVideoRestore(t) {
                    let finished = false, giveUp = null;
                    const onMeta = (e) => {
                        const v = e.target;
                        if (!(v instanceof HTMLVideoElement)) return;
                        if (!isFinite(v.duration) || v.duration < 60 || adActive()) return;
                        if (t < v.duration - 10) {
                            const p = document.getElementById('movie_player');
                            try { if (p && typeof p.seekTo === 'function') p.seekTo(t, true); else v.currentTime = t; } catch (err) {}
                        }
                        finish();
                    };
                    const finish = () => {
                        if (finished) return;
                        finished = true; videoDone = true;
                        document.removeEventListener('loadedmetadata', onMeta, true);
                        clearTimeout(giveUp);
                    };
                    giveUp = setTimeout(finish, 30000);
                    document.addEventListener('loadedmetadata', onMeta, true); // media-события не всплывают, поэтому capture
                }

                (function restoreOnLoad() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const states = readStates();
                    const key = stateKey();
                    const st = states[key];
                    if (!st) return;
                    const age = Date.now() - (st.ts || 0);
                    const bySwitch = !!st.sw && age <= STATE_TTL;
                    // После перезапуска: запись сделана в прошлом запуске, а страница была открыта вкладкой
                    const byRestart = RESTORE_KEYS.has(key) && st.run !== RUN_ID && age <= RESTART_TTL;
                    if (!bySwitch && !byRestart) return;
                    st.sw = false; st.run = RUN_ID; writeStates(states); // флаг сброшен: перезагрузка или переход по ссылке не восстанавливают
                    if (st.f && typeof st.f === 'object') startFormRestore(st.f);
                    if (st.y > 0 || st.x > 0) { scrollDone = false; startScrollRestore(st.x || 0, st.y || 0); }
                    if (st.t > 5 && !window.location.pathname.startsWith('/shorts')) { videoDone = false; startVideoRestore(st.t); }
                })();

                window.addEventListener('scroll', () => {
                    if (scrollTimer) clearTimeout(scrollTimer);
                    const k = stateKey();
                    scrollTimer = setTimeout(() => { scrollTimer = null; if (stateKey() === k) saveState(); }, 400);
                }, { passive: true });
                window.addEventListener('pagehide', () => saveState());
                // Закрытие окна крестиком не гарантирует pagehide, поэтому сохраняем заранее:
                // текст - через 0.8 с после ввода, позицию видео - раз в 10 с, пока оно играет.
                let inputTimer = null;
                document.addEventListener('input', (e) => {
                    if (!e.isTrusted || (ui && e.target && ui.container.contains(e.target))) return;
                    if (inputTimer) clearTimeout(inputTimer);
                    inputTimer = setTimeout(() => { inputTimer = null; saveState(); }, 800);
                }, { capture: true, passive: true });
                setInterval(() => {
                    if (document.hidden) return;
                    const v = mainVideo();
                    if (v && !v.paused) saveState();
                }, 10000);
                document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') saveState(); });
                // Вызывается из Rust прямо перед сменой вкладки
                lockGlobal('__ulSaveForSwitch', () => { switching = true; saveState(); });

                // Горячие клавиши регистрируются ОДИН раз (раньше каждая пересборка UI добавляла ещё один обработчик)
                window.addEventListener('keydown', (e) => {
                    if (!ui || !e.isTrusted) return; // синтетические нажатия, созданные страницей, игнорируем
                    const k = e.key.toLowerCase();
                    // Alt+← / Alt+→ и F5 / Ctrl+R
                    if (e.altKey && !e.ctrlKey) {
                        if (e.key === 'ArrowLeft') { e.preventDefault(); history.back(); }
                        else if (e.key === 'ArrowRight') { e.preventDefault(); history.forward(); }
                        return;
                    }
                    if (e.key === 'F5' || (e.ctrlKey && !e.shiftKey && k === 'r')) { e.preventDefault(); location.reload(); return; }
                    if (e.ctrlKey && e.shiftKey && !e.altKey) {
                        if (k === 'x') { e.preventDefault(); startPick(); return; }
                        if (k === 'z') { e.preventDefault(); undoRule(); return; }
                    }
                    if (!e.ctrlKey) return;
                    if (k === 'l') { e.preventDefault(); ui.show(); ui.input.focus(); }
                    else if (k === 't') { e.preventDefault(); ipcPost('new_tab'); }
                    else if (k === 'w') { e.preventDefault(); ipcPost('CLOSE_TAB|-1'); }
                    else if (k === 'tab') { e.preventDefault(); ipcPost('next_tab'); }
                }, true);

                // Возвращает уже существующие элементы в DOM (а не создаёт новые)
                function attachUI() {
                    const root = document.documentElement;
                    if (!root) return;
                    if (!ui) buildUI();
                    if (!ui.trigger.isConnected) root.appendChild(ui.trigger);
                    if (!ui.container.isConnected) root.appendChild(ui.container);
                }

                // MutationObserver возвращает панель мгновенно, как только сайт её убрал
                let observedRoot = null, observedHead = null, docObserved = false;
                // Возвращает на место панель и наши стили, если сайт их убрал. Дёшево (несколько getElementById)
                // и вызывается только при изменении прямых детей <html> / <head>, а не по таймеру.
                function keepAlive() {
                    if (!ui || !ui.container.isConnected || !ui.trigger.isConnected) attachUI();
                    injectCss();
                    applyCosmetic();
                    ensureUserCss();
                    observeHead();
                }
                function observeHead() {
                    const h = document.head;
                    if (h && h !== observedHead) {
                        observedHead = h;
                        new MutationObserver(keepAlive).observe(h, { childList: true });
                    }
                }
                function watch() {
                    const root = document.documentElement;
                    if (!root) { requestAnimationFrame(watch); return; }
                    attachUI();
                    // Сайт мог заменить сам <html> (document.open / write): у документа 1-2 ребёнка, событий мало
                    if (!docObserved) {
                        docObserved = true;
                        new MutationObserver(() => { if (document.documentElement !== observedRoot) watch(); }).observe(document, { childList: true });
                    }
                    observeHead();
                    if (root === observedRoot) return;
                    observedRoot = root;
                    new MutationObserver(keepAlive).observe(root, { childList: true });
                }
                watch();

                // (Раньше здесь был сторож setInterval раз в секунду на каждой странице - заменён наблюдателями выше.)

                // ---- Контекстное меню (правая кнопка): ссылка, изображение, выделенный текст, видео, страница ----
                // Не вмешиваемся: в полях ввода (там штатное меню WebView2: вставка, орфография), при Shift+правой кнопке
                // и когда сайт показывает своё меню (event.defaultPrevented: Google Docs, плеер YouTube и т.п.).
                let ctxMenu = null;
                function closeCtx() {
                    if (!ctxMenu) return;
                    ctxMenu.remove(); ctxMenu = null;
                    window.removeEventListener('mousedown', ctxOutside, true);
                    window.removeEventListener('keydown', ctxKey, true);
                    window.removeEventListener('scroll', closeCtx, true);
                    window.removeEventListener('blur', closeCtx);
                    window.removeEventListener('resize', closeCtx);
                }
                function ctxOutside(e) { if (ctxMenu && !ctxMenu.contains(e.target)) closeCtx(); }
                function ctxKey(e) { if (e.key === 'Escape') { e.preventDefault(); closeCtx(); } }

                function copyText(t) {
                    const fallback = () => {
                        try {
                            const ta = document.createElement('textarea');
                            ta.value = t; ta.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0;';
                            (document.body || document.documentElement).appendChild(ta);
                            ta.select(); document.execCommand('copy'); ta.remove();
                        } catch (e) {}
                    };
                    try {
                        if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(t).catch(fallback);
                        else fallback();
                    } catch (e) { fallback(); }
                }
                function openInWindow(u) {
                    try {
                        const abs = new URL(u, window.location.href).href;
                        if (/^https?:\/\//i.test(abs)) ipcPost('OPEN_WINDOW|' + abs);
                    } catch (e) {}
                }

                function showCtx(e) {
                    closeCtx();
                    const t = e.target;
                    const items = []; // { label, fn }, null = разделитель
                    const sep = () => { if (items.length && items[items.length - 1] !== null) items.push(null); };
                    const isHttp = (u) => /^https?:\/\//i.test(u);

                    // Ссылка
                    const link = t.closest ? t.closest('a[href], area[href]') : null;
                    if (link) {
                        let abs = '';
                        try { abs = new URL(typeof link.href === 'string' ? link.href : link.getAttribute('href'), window.location.href).href; } catch (err) {}
                        if (abs) {
                            if (isHttp(abs)) {
                                items.push({ label: 'Открыть ссылку в новой вкладке', fn: () => openInTab(abs, true) });
                                items.push({ label: 'Открыть ссылку в новом окне', fn: () => openInWindow(abs) });
                            }
                            items.push({ label: 'Копировать адрес ссылки', fn: () => copyText(abs) });
                            sep();
                        }
                    }
                    // Изображение
                    const img = t.closest ? t.closest('img') : null;
                    const src = img ? (img.currentSrc || img.src || '') : '';
                    if (src && isHttp(src)) {
                        items.push({ label: 'Открыть изображение в новой вкладке', fn: () => openInTab(src, true) });
                        items.push({ label: 'Копировать адрес изображения', fn: () => copyText(src) });
                        sep();
                    }
                    // Выделенный текст
                    let sel = '';
                    try { sel = String(window.getSelection ? window.getSelection() : '').trim(); } catch (err) {}
                    if (sel) {
                        const short = sel.length > 28 ? sel.slice(0, 28) + '…' : sel;
                        items.push({ label: 'Копировать', fn: () => copyText(sel) });
                        items.push({ label: 'Найти «' + short.replace(/\s+/g, ' ') + '» в DuckDuckGo', fn: () => openInTab('https://duckduckgo.com/?q=' + encodeURIComponent(sel.slice(0, 200)), false) });
                        sep();
                    }
                    // Видео: картинка в картинке (под оверлеями плеера ищем видео по координатам)
                    let vid = null;
                    try { vid = (document.elementsFromPoint(e.clientX, e.clientY) || []).find((el) => el.tagName === 'VIDEO') || null; } catch (err) {}
                    if (vid && document.pictureInPictureEnabled && !vid.disablePictureInPicture) {
                        items.push({
                            label: document.pictureInPictureElement === vid ? 'Выйти из режима «картинка в картинке»' : 'Картинка в картинке',
                            fn: () => {
                                if (document.pictureInPictureElement === vid) document.exitPictureInPicture().catch(() => {});
                                else vid.requestPictureInPicture().catch(() => {});
                            }
                        });
                        sep();
                    }
                    // Страница
                    items.push({ label: 'Назад', fn: () => history.back() });
                    items.push({ label: 'Вперёд', fn: () => history.forward() });
                    items.push({ label: 'Обновить', fn: () => location.reload() });
                    sep();
                    items.push({ label: 'Печать…', fn: () => window.print() });
                    items.push({ label: 'Выделить всё', fn: () => { try { window.getSelection().selectAllChildren(document.body || document.documentElement); } catch (err) {} } });
                    items.push({ label: 'Копировать адрес страницы', fn: () => copyText(window.location.href) });
                    items.push({ label: 'Вывести текущую вкладку в отдельное окно', fn: () => ipcPost('DETACH_TAB') });
                    while (items.length && items[items.length - 1] === null) items.pop();

                    const m = document.createElement('div');
                    m.style.cssText = 'position:fixed;left:0;top:0;z-index:2147483647;min-width:230px;max-width:360px;padding:6px 0;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.5);font:13px/1.4 "Segoe UI",system-ui,sans-serif;user-select:none;-webkit-user-select:none;';
                    m.addEventListener('mousedown', (ev) => ev.preventDefault()); // клик по меню не снимает выделение
                    m.addEventListener('contextmenu', (ev) => ev.preventDefault());
                    items.forEach((it) => {
                        const row = document.createElement('div');
                        if (it === null) {
                            row.style.cssText = 'height:1px;margin:6px 0;background:#3c4043;';
                        } else {
                            row.textContent = it.label;
                            row.style.cssText = 'padding:6px 18px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;';
                            row.onmouseover = () => { row.style.background = '#303134'; };
                            row.onmouseout = () => { row.style.background = 'transparent'; };
                            row.onclick = (ev) => { closeCtx(); if (!ev.isTrusted) return; try { it.fn(); } catch (err) {} };
                        }
                        m.appendChild(row);
                    });
                    (document.documentElement || document.body).appendChild(m);
                    const w = m.offsetWidth, h = m.offsetHeight;
                    m.style.left = Math.max(4, Math.min(e.clientX, window.innerWidth - w - 4)) + 'px';
                    m.style.top = Math.max(4, Math.min(e.clientY, window.innerHeight - h - 4)) + 'px';
                    ctxMenu = m;
                    window.addEventListener('mousedown', ctxOutside, true);
                    window.addEventListener('keydown', ctxKey, true);
                    window.addEventListener('scroll', closeCtx, true);
                    window.addEventListener('blur', closeCtx);
                    window.addEventListener('resize', closeCtx);
                }

                window.addEventListener('contextmenu', (e) => {
                    if (e.defaultPrevented || e.shiftKey) return;
                    const t = e.target;
                    if (!t || t.nodeType !== 1) return;
                    if (ui && ui.container.contains(t)) { e.preventDefault(); return; } // на собственной панели меню не нужно
                    if (t.isContentEditable || (t.closest && t.closest('input, textarea, select'))) return; // штатное меню
                    e.preventDefault();
                    showCtx(e);
                }, false);

                // ---- Замеры загрузки (PERF_LOG в Rust): время работы этого скрипта и этапы загрузки страницы ----
                if (PERF && window === window.top) {
                    const initMs = performance.now() - perfT0;
                    window.addEventListener('load', () => setTimeout(() => {
                        try {
                            const n = performance.getEntriesByType('navigation')[0];
                            if (!n) return;
                            ipcPost('PERF|' + Math.round(initMs) + '|' + Math.round(n.responseStart) + '|' + Math.round(n.domContentLoadedEventEnd) + '|' + Math.round(n.loadEventEnd) + '|' + window.location.hostname);
                        } catch (e) {}
                    }, 0));
                }

            } catch (err) { console.error("Script error: ", err); }
        })();
    "#.replace("__RUN_ID__", &run_id).replace("__RESTORE_URLS__", &restore_urls_js)
        .replace("__PERF__", if PERF_LOG { "true" } else { "false" })
        .replace("__IPC_TOKEN__", &ipc_secret)
        .replace("__NATIVE__", if cfg!(all(windows, feature = "native-adblock")) { "true" } else { "false" })
        .replace("__AD_HOSTS__", &ad_hosts_js);

    // Косметические фильтры - отдельный initialization script только для верхнего окна: во фреймах он
    // не выполняет ничего, а сама огромная строка лежит внутри функции, которую никто не вызывает
    // (движок не создаёт для неё строку). Должен идти в списке скриптов ПЕРЕД основным.
    let cosmetic_script = format!(
        "if (window === window.top) {{ window.__ulCosmRaw = function () {{ return '{}'; }}; }}",
        js_str(&cosmetic_js)
    );

// Получаем путь C:\Users\<Имя>\AppData\Local\Falco\User Data
    let profile_path = app_dir.join("User Data");

    let mut web_context = WebContext::new(Some(profile_path));


    let proxy_waiter = proxy.clone();
    let proxy_newwin = proxy.clone();
    let builder = WebViewBuilder::new(&window)
        .with_autoplay(true)
        .with_devtools(false)
        .with_web_context(&mut web_context)
        .with_initialization_script(&cosmetic_script)
        .with_initialization_script(&script)
        .with_ipc_handler(move |msg: String| {
            // Принимаем только сообщения с секретом запуска и не длиннее лимита. Всё остальное - это
            // страница сайта (или рекламный iframe), которая пытается сама вызвать window.chrome.webview.postMessage
            if msg.len() > MAX_IPC_LEN {
                return;
            }
            if let Some(rest) = msg.strip_prefix(ipc_prefix.as_str()) {
                let _ = proxy.send_event(rest.to_string());
            }
        })
        // Переходы только на http(s) и служебные адреса: file:, ms-msdt:, search-ms: и подобное отменяются
        .with_navigation_handler(|url: String| nav_allowed(&url))
        // Нативные окна не открываются никогда: адрес уходит в список вкладок как фоновая вкладка
        // Любые попытки открыть нативное окно в обход нашего JS-перехватчика блокируем
        .with_new_window_req_handler(|_url: String| false)
        .with_url(&start_url).unwrap();

    // Каждый пользовательский скрипт - отдельный initialization script (ошибка в одном не ломает остальные)
    let mut builder = builder;
    for s in &user_scripts {
        builder = builder.with_initialization_script(s);
    }

    // Windows: домены из filters.dns (встроенные + начало списков) не резолвятся вообще (script, iframe, img, XHR - всё).
    // Стандартные аргументы wry указаны явно, т.к. свои аргументы их заменяют.
    #[cfg(windows)]
    let builder = {
        let rules = dns_rules;
        let mut disabled = String::from("msWebOOUI,msPdfOOUI");
        if !SMARTSCREEN {
            disabled.push_str(",msSmartScreenProtection");
        }
        let webrtc = if WEBRTC_HIDE_LOCAL_IP { " --force-webrtc-ip-handling-policy=default_public_interface_only" } else { "" };
        builder.with_additional_browser_args(format!(
            "--disable-features={} --autoplay-policy=no-user-gesture-required{} --host-resolver-rules=\"{}\"",
            disabled, webrtc, rules
        ))
    };

    let webview = builder.build().unwrap();

    // Камера и микрофон (ошибка установки не должна ронять браузер: пишем её в файл рядом с профилем)
    #[cfg(windows)]
    {
        if MEDIA_MODE != 0 {
            let trusted = load_trusted_media(&trusted_media_path);
            if let Err(e) = media_permissions::install(&webview, trusted) {
                let _ = fs::write(app_dir.join("media-permissions.log"), format!("{:?}", e));
            }
        }
    }
    // WebView2 уже получил скрипты, наши копии больше не нужны
    drop(script);
    drop(cosmetic_script);
    drop(user_scripts);

    // Нативный блокировщик (cargo build --release --features native-adblock)
    #[cfg(all(windows, feature = "native-adblock"))]
    let page_url = std::rc::Rc::new(std::cell::RefCell::new(start_url.clone()));
    #[cfg(all(windows, feature = "native-adblock"))]
    {
        let (engine, n) = native_adblock::build_engine(&filters_dir);
        let dbg = app_dir.join("native-adblock-debug.log");
        if native_adblock::DEBUG {
            let _ = fs::write(&dbg, format!("filter lines loaded: {}\n", n));
        }
        match native_adblock::install(&webview, engine, page_url.clone(), dbg.clone()) {
            Ok(()) => {
                if native_adblock::DEBUG {
                    native_adblock::log(&dbg, "handler installed OK\n");
                }
            }
            // Ошибка установки не должна ронять браузер: пишем её в файл рядом с профилем
            Err(e) => {
                let _ = fs::write(app_dir.join("native-adblock.log"), format!("{:?}", e));
            }
        }
    }

    let mut tabs: Vec<String> = tabs_init;
    let mut current_tab = current_init;
    let mut last_saved = String::new();
    if let Some(sp) = &save_path {
        save_session(sp, &mut last_saved, &tabs, current_tab);
    }
    let mut popout_alive: u32 = 0; // сколько отдельных окон сейчас открыто из этого окна
    let mut popout_seq: u32 = 0;
    let mut switch_at: Option<std::time::Instant> = None; // когда начали смену вкладки (для perf.log)
    let mut perf_lines: u32 = 0;
    let mut rl_start = std::time::Instant::now(); // окно подсчёта команд в секунду
    let mut rl_count: u32 = 0;
    let mut last_spawn: Option<std::time::Instant> = None; // последний запуск отдельного окна
    let mut last_newwin: Option<std::time::Instant> = None; // последняя вкладка от "нативного" окна сайта

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if let Some(sp) = &save_path {
                    save_session(sp, &mut last_saved, &tabs, current_tab);
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(msg) => {
                // Защита от флуда: не больше MAX_IPC_PER_SEC команд в секунду (SYNC_URL и служебные события не считаем)
                if !msg.starts_with("SYNC_URL|") && !msg.starts_with("POPOUT_CLOSED|") {
                    let now = std::time::Instant::now();
                    if now.duration_since(rl_start).as_millis() >= 1000 {
                        rl_start = now;
                        rl_count = 0;
                    }
                    rl_count += 1;
                    if rl_count > MAX_IPC_PER_SEC {
                        return;
                    }
                }
                // В список вкладок попадают только http(s)-адреса (не javascript:, file:, data:)
                if let Some(u) = msg.strip_prefix("SYNC_URL|") {
                    if !is_http_url(u) {
                        return;
                    }
                }
                if PERF_LOG
                    && (msg.starts_with("SWITCH_TAB|") || msg == "next_tab" || msg == "new_tab" || msg.starts_with("OPEN_TAB|"))
                {
                    switch_at = Some(std::time::Instant::now());
                }
                if msg.starts_with("SYNC_URL|") {
                    let url = msg["SYNC_URL|".len()..].to_string();
                    if PERF_LOG {
                        // SYNC_URL приходит в самом начале загрузки новой страницы: это момент, когда она "началась"
                        if let Some(t) = switch_at.take() {
                            perf_log(&app_dir, &format!("switch->page started: {} ms  ({})", t.elapsed().as_millis(), url.split('/').nth(2).unwrap_or("")));
                        }
                    }
                    #[cfg(all(windows, feature = "native-adblock"))]
                    {
                        *page_url.borrow_mut() = url.clone();
                    }
                    tabs[current_tab] = url;
                } else if msg == "GET_TABS" {
                    let tabs_str = tabs_labels(&tabs);
                    let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                    let _ = webview.evaluate_script(&js);
                } else if msg == "new_tab" && tabs.len() < MAX_TABS {
                    tabs.push("https://duckduckgo.com".to_string());
                    current_tab = tabs.len() - 1;
                    let _ = webview.evaluate_script(&nav_script("https://duckduckgo.com", true));
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB|") {
                    // Новое окно сайта (target="_blank", window.open) -> новая вкладка, сразу переходим на неё
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        current_tab = tabs.len() - 1;
                        let _ = webview.evaluate_script(&nav_script(url, true));
                    }
                } else if let Some(url) = msg.strip_prefix("OPEN_WINDOW|") {
                    // "Открыть ссылку в новом окне": отдельное окно без возврата вкладок (в отличие от DETACH_TAB)
                    if is_http_url(url) && popout_alive < 5 && spawn_ok(&mut last_spawn) {
                        popout_seq += 1;
                        let id = format!("{}_{}", std::process::id(), popout_seq);
                        let spawned = std::env::current_exe().and_then(|exe| {
                            std::process::Command::new(exe).arg(format!("--app={}", url)).spawn()
                        });
                        if let Ok(mut child) = spawned {
                            popout_alive += 1;
                            let p = proxy_waiter.clone();
                            std::thread::spawn(move || {
                                let _ = child.wait();
                                let _ = p.send_event(format!("POPOUT_CLOSED|{}", id));
                            });
                        }
                    }
                } else if let Some(url) = msg.strip_prefix("NEWWIN|") {
                    // Сайт попытался открыть нативное окно мимо нашего JS (форма с target=_blank и т.п.).
                    // Окно не открывается; адрес становится ФОНОВОЙ вкладкой (текущую страницу не покидаем), не чаще раза в 1,5 с
                    if is_http_url(url) && tabs.len() < MAX_TABS && spawn_ok(&mut last_newwin) {
                        tabs.push(url.to_string());
                        let tabs_str = tabs_labels(&tabs);
                        let js = format!(
                            "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulFlash) window.__ulFlash();",
                            js_str(&tabs_str),
                            current_tab
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB_BG|") {
                    // Фоновая вкладка (колесо, Ctrl+клик): добавляем в список, текущую страницу не трогаем
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        let tabs_str = tabs_labels(&tabs);
                        let js = format!(
                            "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulFlash) window.__ulFlash();",
                            js_str(&tabs_str),
                            current_tab
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(rest) = msg.strip_prefix("PERF|") {
                    if PERF_LOG && perf_lines < 500 {
                        perf_lines += 1;
                        let clean: String = rest.chars().filter(|c| !c.is_control()).take(200).collect();
                        perf_log(&app_dir, &format!("page: init|ttfb|dcl|load|host = {}", clean));
                    }
                } else if msg.starts_with("SWITCH_TAB|") {
                    if let Ok(idx) = msg.replace("SWITCH_TAB|", "").parse::<usize>() {
                        if idx < tabs.len() {
                            current_tab = idx;
                            let target_url = &tabs[current_tab];
                            let _ = webview.evaluate_script(&nav_script(target_url, true));
                        }
                    }
                } else if msg.starts_with("CLOSE_TAB|") {
                    let mut idx_to_close = current_tab;
                    let parsed = msg.replace("CLOSE_TAB|", "").parse::<i32>().unwrap_or(-1);
                    if parsed >= 0 { idx_to_close = parsed as usize; }
                    
                    if tabs.len() > 1 && idx_to_close < tabs.len() {
                        let was_active = idx_to_close == current_tab;
                        tabs.remove(idx_to_close);
                        
                        if current_tab > idx_to_close { current_tab -= 1; } 
                        else if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                        
                        if was_active {
                            let target_url = &tabs[current_tab];
                            let _ = webview.evaluate_script(&nav_script(target_url, false));
                        } else {
                            let tabs_str = tabs_labels(&tabs);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    } else if tabs.len() <= 1 {
                        *control_flow = ControlFlow::Exit;
                    }
                } else if msg == "DETACH_TAB" {
                    // Новое окно = второй запуск программы с --app=<адрес вкладки> (сессию он не трогает).
                    // Только если оно реально запустилось, вкладку убираем из этого окна: страница выгружается,
                    // поэтому звонок здесь завершается, а в новом окне открывается заново.
                    // Лимит окон защищает от страниц, которые могли бы спамить этой командой.
                    let url = tabs[current_tab].clone();
                    if is_http_url(&url) && popout_alive < 5 && spawn_ok(&mut last_spawn) {
                        popout_seq += 1;
                        let id = format!("{}_{}", std::process::id(), popout_seq);
                        let spawned = std::env::current_exe().and_then(|exe| {
                            std::process::Command::new(exe)
                                .arg(format!("--app={}", url))
                                .arg(format!("--return={}", id))
                                .spawn()
                        });
                        if let Ok(mut child) = spawned {
                            popout_alive += 1;
                            // Поток спит, пока окно не закроется (процессор не тратит), и затем сообщает об этом
                            let p = proxy_waiter.clone();
                            std::thread::spawn(move || {
                                let _ = child.wait();
                                let _ = p.send_event(format!("POPOUT_CLOSED|{}", id));
                            });
                            if tabs.len() > 1 {
                                tabs.remove(current_tab);
                                if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                                let target_url = &tabs[current_tab];
                                let _ = webview.evaluate_script(&nav_script(target_url, false));
                            } else {
                                // единственная вкладка: в этом окне остаётся стартовая страница
                                tabs[0] = DEFAULT_URL.to_string();
                                current_tab = 0;
                                let _ = webview.evaluate_script(&nav_script(DEFAULT_URL, false));
                            }
                        }
                    }
                } else if let Some(id) = msg.strip_prefix("POPOUT_CLOSED|") {
                    // Отдельное окно закрыто: его вкладки добавляем в конец списка (текущую вкладку не переключаем)
                    if id.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
                        popout_alive = popout_alive.saturating_sub(1);
                        let path = app_dir.join(format!("popout-{}.txt", id));
                        let (back, _) = load_session(&path);
                        let _ = fs::remove_file(&path);
                        let _ = fs::remove_file(path.with_extension("tmp"));
                        let mut added = false;
                        for u in back {
                            if tabs.len() < MAX_TABS && !tabs.contains(&u) {
                                tabs.push(u);
                                added = true;
                            }
                        }
                        if added {
                            let tabs_str = tabs_labels(&tabs);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg == "next_tab" {
                    if tabs.len() > 1 {
                        current_tab = (current_tab + 1) % tabs.len();
                        let target_url = &tabs[current_tab];
                        let _ = webview.evaluate_script(&nav_script(target_url, true));
                    }
                }
                if msg != "GET_TABS" {
                    if let Some(sp) = &save_path {
                        save_session(sp, &mut last_saved, &tabs, current_tab);
                    }
                }
            }
            _ => {}
        }
    });
}
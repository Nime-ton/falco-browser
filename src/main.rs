#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

#[cfg(windows)]
use tao::platform::windows::WindowBuilderExtWindows;
#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

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

const GOOGLE_AD_HOSTS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "adservice.google.com", "google-analytics.com",
];

const NEVER_BLOCK: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "googlevideo.com", "ytimg.com", "ggpht.com",
    "gstatic.com", "google.com", "googleapis.com", "googleusercontent.com", "gvt1.com", "gvt2.com",
];
const NO_DNS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "google-analytics.com",
    "googletagmanager.com", "googletagservices.com", "2mdn.net", "adservice.google.com",
];
const DNS_BUDGET: usize = 20_000;
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
    !d.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

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
            continue;
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

    pub const DEBUG: bool = false;

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
        match super::host_of(page) {
            Some(h) => super::is_under(&h, "youtube.com") || super::is_under(&h, "youtube-nocookie.com"),
            None => false,
        }
    }

    fn is_local_url(url: &str) -> bool {
        for base in ["http://127.0.0.1", "http://localhost"] {
            if let Some(rest) = url.strip_prefix(base) {
                if rest.is_empty() || rest.starts_with(':') || rest.starts_with('/') {
                    return true;
                }
            }
        }
        false
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
                if is_local_url(&url) {
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

fn nav_script(url: &str, save_first: bool) -> String {
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

const MAX_TABS: usize = 50;
const MAX_CLOSED_TABS: usize = 25;
const MAX_URL_LEN: usize = 4096;
const MAX_IPC_LEN: usize = 8192;
const MAX_IPC_PER_SEC: u32 = 60;
const SMARTSCREEN: bool = true;
const WEBRTC_HIDE_LOCAL_IP: bool = true;

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

fn host_of(url: &str) -> Option<String> {
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

fn tab_label(url: &str) -> String {
    match host_of(url) {
        Some(h) => h.strip_prefix("www.").map(|x| x.to_string()).unwrap_or(h),
        None => "Новая вкладка".to_string(),
    }
}

fn tabs_labels(tabs: &[String], titles: &HashMap<String, String>) -> String {
    tabs.iter()
        .map(|u| match titles.get(u) {
            Some(t) if !t.is_empty() => t.clone(),
            _ => tab_label(u),
        })
        .collect::<Vec<_>>()
        .join("|||")
}

fn nav_allowed(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "about" | "blob" | "data" | "chrome-error"
        ),
        None => false,
    }
}

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

const PERF_LOG: bool = false;

fn perf_log(dir: &Path, line: &str) {
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
const DEFAULT_URL: &str = "https://duckduckgo.com";

fn is_http_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() > 8
        && s.len() <= MAX_URL_LEN
        && !s.bytes().any(|b| b <= b' ' || b == 0x7f)
        && !s.contains("|||")
}

// ====================== Положение и размер окна ======================
struct WinState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    max: bool,
}

fn load_win_state(path: &Path) -> Option<WinState> {
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

fn save_win_state(path: &Path, st: &WinState) {
    let s = format!("{} {} {} {} {}\n", st.x, st.y, st.w, st.h, if st.max { 1 } else { 0 });
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

// ====================== История, закладки и загрузки ======================
const HISTORY_ENABLED: bool = true;
const HISTORY_SKIP_LOCAL: bool = true;
const HISTORY_MAX: usize = 3000;
const HISTORY_SHOW: usize = 400;
const BOOKMARKS_MAX: usize = 2000;
const DOWNLOADS_MAX: usize = 500;
const FIELD_SEP: char = '\u{1f}';

struct Entry {
    ts: u64,
    url: String,
    title: String,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clean_title(t: &str) -> String {
    t.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string()
}

fn is_local_host(url: &str) -> bool {
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

fn read_entries(path: &Path) -> Vec<Entry> {
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

fn write_entries(path: &Path, list: &[Entry]) {
    let mut s = String::new();
    for e in list {
        s.push_str(&format!("{}{}{}{}{}\n", e.ts, FIELD_SEP, e.url, FIELD_SEP, e.title));
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn append_entry(path: &Path, e: &Entry) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}{}{}{}{}", e.ts, FIELD_SEP, e.url, FIELD_SEP, e.title);
    }
}

fn history_view(path: &Path, limit: usize) -> Vec<Entry> {
    let mut all = read_entries(path);
    all.reverse();
    let mut seen: HashSet<String> = HashSet::new();
    all.retain(|e| seen.insert(e.url.clone()));
    all.truncate(limit);
    all
}

fn compact_history(path: &Path) {
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len < 600_000 {
        return;
    }
    let mut keep = history_view(path, HISTORY_MAX);
    keep.reverse();
    write_entries(path, &keep);
}

fn history_del(path: &Path, url: &str) {
    let mut list = read_entries(path);
    let before = list.len();
    list.retain(|e| e.url != url);
    if list.len() != before {
        write_entries(path, &list);
    }
}

struct HistState {
    pending: Option<Entry>,
    last: Option<(String, String)>,
}

impl HistState {
    fn new() -> Self {
        HistState { pending: None, last: None }
    }

    fn note(&mut self, path: &Path, url: &str, title: &str) {
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

    fn leave(&mut self, path: &Path, new_url: &str) {
        let other = self.pending.as_ref().map_or(false, |p| p.url != new_url);
        if other {
            self.flush(path);
        }
    }

    fn flush(&mut self, path: &Path) {
        if let Some(e) = self.pending.take() {
            if !HISTORY_ENABLED || e.title.is_empty() || !is_http_url(&e.url) {
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

fn bm_add(path: &Path, url: &str, title: &str) -> String {
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

fn bm_rename(path: &Path, url: &str, title: &str) -> bool {
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

fn bm_state_js(map: &HashMap<String, String>, url: &str) -> String {
    match map.get(url) {
        Some(t) => format!("if(window.__ulBM) window.__ulBM(true,'{}');", js_str(t)),
        None => "if(window.__ulBM) window.__ulBM(false);".to_string(),
    }
}

fn bm_del(path: &Path, url: &str) {
    history_del(path, url);
}

fn bm_view(path: &Path) -> Vec<Entry> {
    let mut list = read_entries(path);
    list.reverse();
    list
}

// ====================== Загрузки ======================
fn default_download_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Downloads"))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Downloads")))
        .unwrap_or_else(std::env::temp_dir)
}

fn unique_download_path(proposed: &Path) -> PathBuf {
    let dir = proposed
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_download_dir);
    let _ = fs::create_dir_all(&dir);

    let file_name = proposed
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = proposed.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    let ext = proposed.extension().and_then(|e| e.to_str()).unwrap_or("");
    for i in 1..10_000 {
        let new_name = if ext.is_empty() {
            format!("{} ({})", stem, i)
        } else {
            format!("{} ({}).{}", stem, i, ext)
        };
        let p = dir.join(new_name);
        if !p.exists() {
            return p;
        }
    }
    candidate
}

fn read_downloads(path: &Path) -> Vec<Entry> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, FIELD_SEP);
            let ts = it.next()?.parse::<u64>().ok()?;
            let file_path = it.next()?.trim();
            if file_path.is_empty() || file_path.len() > MAX_URL_LEN || file_path.chars().any(|c| c.is_control()) {
                return None;
            }
            let title = clean_title(it.next().unwrap_or(""));
            Some(Entry {
                ts,
                url: file_path.to_string(),
                title,
            })
        })
        .collect()
}

fn dl_add(path: &Path, file_path: &str, title: &str) {
    let mut list = read_downloads(path);
    list.retain(|e| e.url != file_path);
    if list.len() >= DOWNLOADS_MAX {
        list.remove(0);
    }
    list.push(Entry {
        ts: now_secs(),
        url: file_path.to_string(),
        title: clean_title(title),
    });
    write_entries(path, &list);
}

fn dl_del(path: &Path, file_path: &str) {
    let mut list = read_downloads(path);
    let before = list.len();
    list.retain(|e| e.url != file_path);
    if list.len() != before {
        write_entries(path, &list);
    }
}

fn dl_view(path: &Path) -> Vec<Entry> {
    let mut list = read_downloads(path);
    list.reverse();
    list
}

fn open_downloads_folder() {
    let dir = default_download_dir();
    let _ = fs::create_dir_all(&dir);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
}

fn show_in_folder(file_path: &str) {
    let p = Path::new(file_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if p.exists() {
            let full = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            let clean = full.to_string_lossy();
            let clean = clean.strip_prefix(r"\\?\").unwrap_or(&clean);
            let _ = std::process::Command::new("explorer.exe")
                .raw_arg(format!("/select,\"{}\"", clean))
                .spawn();
        } else if let Some(parent) = p.parent().filter(|d| d.exists()) {
            let _ = std::process::Command::new("explorer.exe").arg(parent).spawn();
        } else {
            open_downloads_folder();
        }
    }
    #[cfg(target_os = "macos")]
    {
        if p.exists() {
            let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
        } else {
            open_downloads_folder();
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = p.parent().filter(|d| d.exists()) {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        } else {
            open_downloads_folder();
        }
    }
}

fn json_quote(s: &str) -> String {
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

fn entries_json(list: &[Entry]) -> String {
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

fn panel_js(kind: &str, list: &[Entry]) -> String {
    format!("if(window.__ulPanel) window.__ulPanel('{}','{}');", kind, js_str(&entries_json(list)))
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

    let app_dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local")))
        .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())))
        .unwrap_or_else(std::env::temp_dir)
        .join("Falco");

    let cli_url = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--app=").map(|s| s.trim_matches('"').to_string()))
        .or_else(|| std::env::args().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://")))
        .filter(|u| is_http_url(u));
    let session_enabled = cli_url.is_none();

    let win_state_path = app_dir.join("window.txt");
    let mut win_state = if session_enabled {
        load_win_state(&win_state_path).unwrap_or(WinState { x: 100, y: 100, w: 1280, h: 800, max: false })
    } else {
        WinState { x: 120, y: 120, w: 1100, h: 750, max: false }
    };

    let mut wb = WindowBuilder::new()
        .with_title("Falco")
        .with_decorations(false)
        .with_inner_size(PhysicalSize::new(win_state.w, win_state.h));

    #[cfg(windows)]
    {
        wb = wb.with_undecorated_shadow(true);
    }

    if session_enabled {
        wb = wb.with_position(PhysicalPosition::new(win_state.x, win_state.y));
    }

    let window = wb.build(&event_loop).unwrap();

    if session_enabled && win_state.max {
        window.set_maximized(true);
    }

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
    let ipc_secret = make_secret();
    let ipc_prefix = format!("{}:", ipc_secret);
    let filters = load_filters(&filters_dir);
    let user_scripts = load_user_scripts(&scripts_dir);
    let ad_hosts_js = filters.js.join(",");
    let cosmetic_js = cosmetic_json(&filters);
    let dns_rules = filters
        .dns
        .iter()
        .map(|h| format!("MAP {h} ~NOTFOUND, MAP *.{h} ~NOTFOUND"))
        .collect::<Vec<_>>()
        .join(", ");
    drop(filters);

    let session_path = app_dir.join("session.txt");
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

    let ret_id: Option<String> = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--return=").map(|s| s.to_string()))
        .filter(|s| !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_digit() || b == b'_'));
    let save_path: Option<PathBuf> = if session_enabled {
        Some(session_path)
    } else {
        ret_id.map(|id| app_dir.join(format!("popout-{}.txt", id)))
    };
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
    let restore_urls_js = if restored { json_arr(&tabs_init) } else { "[]".to_string() };
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();

    let script = r#"
        (function() {
            try {
                const PERF = __PERF__;
                const NATIVE = __NATIVE__;

                // ---- Защищённый канал связи с Rust ----
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
                const lockGlobal = (name, fn) => {
                    try { Object.defineProperty(window, name, { value: fn, writable: false, configurable: false, enumerable: false }); } catch (e) {}
                };
                const rApply = Reflect.apply;
                const rAttachShadow = Element.prototype.attachShadow;
                const rJsonParse = JSON.parse;
                const perfT0 = performance.now();
                if (window === window.top) ipcPost("SYNC_URL|" + window.location.href);

                // 1. БЛОКИРОВЩИК
                const adRegex = /(doubleclick\.net|google-analytics\.com|googlesyndication\.com|adservice\.google\.com|mc\.yandex\.ru|an\.yandex\.ru|amazon-adsystem\.com|appnexus\.com|criteo\.com|taboola\.com|yadro\.ru|top-fwz1\.mail\.ru|propellerads|popads|adk2x|betboom|1xbet|fonbet|melbet|awaps\.|a-ads\.com|mgid\.com|adskeeper|exoclick|clickadu|trafficstars|adfox|adriver|\/pagead\/|\/get_midroll_info)/i;
                const isYT = /(^|\.)youtube(-nocookie)?\.com$/i.test(window.location.hostname);
                function urlOf(x) {
                    try { return typeof x === 'string' ? x : (x && x.url) ? x.url : String(x); } catch (e) { return ''; }
                }
                let AD_SET = null;
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
                    if (isYT) return false;
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
                const perfNow = performance.now.bind(performance);
                const ulLastOpen = { u: '', t: -1e9 };
                let lastTrustedGestureTime = -1e9;
                let lastGestureInteractive = false;

                function isInteractiveTarget(el) {
                    if (!el || el.nodeType !== 1) return false;
                    if (el.closest && el.closest('a, area, button, input, select, textarea, summary, [role="button"], [role="link"], [tabindex]')) return true;
                    try { if (window.getComputedStyle(el).cursor === 'pointer') return true; } catch (e) {}
                    return false;
                }

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
                        lastTrustedGestureTime = -1e9;
                        lastGestureInteractive = false;
                        if ((abs === ulLastOpen.u && now - ulLastOpen.t < 500) || now - ulLastOpen.t < 120) return true;
                        ulLastOpen.u = abs; ulLastOpen.t = now;
                        ipcPost((background ? 'OPEN_TAB_BG|' : 'OPEN_TAB|') + abs);
                        return true;
                    } catch (e) { return false; }
                }

                window.open = function(u) {
                    if (u && userGesture(null)) openInTab(String(u), false);
                    return null;
                };

                const origElemClick = HTMLElement.prototype.click;
                HTMLElement.prototype.click = function() {
                    const tag = this.tagName;
                    if (tag === 'A' || tag === 'AREA') {
                        const tgt = (this.target || '').toLowerCase();
                        if ((!this.isConnected || tgt === '_blank') && !this.hasAttribute('download') && !userGesture(null)) return;
                    }
                    return origElemClick.apply(this, arguments);
                };

                const origFormSubmit = HTMLFormElement.prototype.submit;
                HTMLFormElement.prototype.submit = function() {
                    if (String(this.getAttribute('target') || '').toLowerCase() === '_blank' && !userGesture(null)) return;
                    return origFormSubmit.apply(this, arguments);
                };
                document.addEventListener('submit', (e) => {
                    const f = e.target;
                    if (f && f.getAttribute && String(f.getAttribute('target') || '').toLowerCase() === '_blank' && !userGesture(e)) {
                        e.preventDefault();
                        e.stopImmediatePropagation();
                    }
                }, true);

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
                        e.preventDefault();
                        e.stopImmediatePropagation();
                        return;
                    }
                    const href = typeof a.href === 'string' ? a.href : a.getAttribute('href');
                    if (openInTab(href, mode === 'bg')) e.preventDefault();
                }
                document.addEventListener('click', onLinkClick, true);
                document.addEventListener('auxclick', onLinkClick, true);

                // ---- YouTube ----
                function setupYouTube() {
                    const FORCE_H264 = false;
                    if (FORCE_H264) {
                        const badCodec = /av01|vp0?9/i;
                        if (window.MediaSource && MediaSource.isTypeSupported) {
                            const origSupported = MediaSource.isTypeSupported.bind(MediaSource);
                            MediaSource.isTypeSupported = function(type) { return badCodec.test(String(type)) ? false : origSupported(type); };
                        }
                        const origCanPlay = HTMLMediaElement.prototype.canPlayType;
                        HTMLMediaElement.prototype.canPlayType = function(type) { return badCodec.test(String(type)) ? '' : origCanPlay.apply(this, arguments); };
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

                    let playerRespValue;
                    try {
                        Object.defineProperty(window, 'ytInitialPlayerResponse', {
                            configurable: true,
                            get() { return playerRespValue; },
                            set(v) { try { playerRespValue = prunePlayerAds(v); } catch (e) { playerRespValue = v; } }
                        });
                    } catch (e) {}

                    const YT_PLAYER_API = /\/youtubei\/v1\/player/;
                    const origJson = Response.prototype.json;
                    Response.prototype.json = function() {
                        const p = origJson.apply(this, arguments);
                        if (!YT_PLAYER_API.test(this.url || '')) return p;
                        return p.then((d) => { try { return prunePlayerAds(d); } catch (e) { return d; } });
                    };

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
                    function ytWatch() {
                        if (document.hidden) return;
                        const player = document.getElementById('movie_player');
                        if (player && player !== watchedPlayer) {
                            watchedPlayer = player;
                            if (playerObs) playerObs.disconnect();
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

                    const origAClick = HTMLAnchorElement.prototype.click;
                    HTMLAnchorElement.prototype.click = function() {
                        if (!this.isConnected && this.target === '_blank' && !this.hasAttribute('download')) return;
                        return origAClick.apply(this, arguments);
                    };

                    const AD_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED']);
                    const scanNode = (n) => {
                        if (!n || n.nodeType !== 1) return;
                        if (AD_TAGS.has(n.tagName) && isBlocked(n.src)) { n.remove(); return; }
                        if (n.firstElementChild) {
                            n.querySelectorAll('script[src],iframe[src],img[src],embed[src]').forEach((el) => { if (isBlocked(el.src)) el.remove(); });
                        }
                    };
                    if (!(NATIVE && window === window.top)) {
                        new MutationObserver((muts) => {
                            for (const m of muts) for (const n of m.addedNodes) scanNode(n);
                        }).observe(document, { childList: true, subtree: true });
                    }
                }

                // Стили подсветки найденных слов на странице (CSS Custom Highlight API - без изменения DOM)
                const highlightRules = `
                    ::highlight(ul-find-all) { background-color: rgba(253, 214, 99, 0.42); color: inherit; }
                    ::highlight(ul-find-cur) { background-color: #ff9800; color: #000; }
                `;
                const css = highlightRules + `
                    @view-transition { navigation: auto; }
                    ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
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
                const ytCss = highlightRules + `
                    @view-transition { navigation: auto; }
                    ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
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

                let COSM = null;
                let cosmCss = null, cosmEl = null;
                function applyCosmetic() {
                    if (isYT || window !== window.top) return;
                    if (cosmCss === null) {
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

                let ui = null;

                // ---- URL-синхронизация ----
                let lastSyncedUrl = window.location.href;
                let lastTitle = null, titleTimer = null, titleEl = null, titleObs = null;

                function syncUrl() {
                    if (window !== window.top) return;
                    const href = window.location.href;
                    if (href !== lastSyncedUrl) {
                        lastSyncedUrl = href;
                        ipcPost('SYNC_URL|' + href);
                        lastTitle = null; scheduleTitle();
                    }
                    if (ui && !ui.inputFocused()) ui.input.value = href;
                }
                const onNavigation = syncUrl;

                const origPush = history.pushState;
                history.pushState = function() { const r = origPush.apply(this, arguments); onNavigation(); return r; };
                const origReplace = history.replaceState;
                history.replaceState = function() { const r = origReplace.apply(this, arguments); onNavigation(); return r; };
                window.addEventListener('popstate', onNavigation);
                window.addEventListener('hashchange', onNavigation);
                window.addEventListener('yt-navigate-finish', onNavigation);

                // 2. ИНТЕРФЕЙС
                if (window !== window.top) return;

                // ---- Изолированное масштабирование ТОЛЬКО контента страницы (document.body) ----
                // Верхняя панель вкладок (#light-browser-ui) и плашка уведомлений висят на <html> (documentElement),
                // поэтому они остаются 100% чёткими и не меняют свой размер при зуме страницы!
                const ZOOM_KEY = 'ultralight_page_zoom';
                let pageZoom = 1.0;
                try {
                    const savedZ = parseFloat(localStorage.getItem(ZOOM_KEY) || '1');
                    if (savedZ >= 0.3 && savedZ <= 3.0) pageZoom = savedZ;
                } catch (e) {}

                function applyPageZoom(showNotice) {
                    if (document.body) {
                        document.body.style.zoom = pageZoom === 1.0 ? '' : String(pageZoom);
                    }
                    try {
                        if (pageZoom === 1.0) localStorage.removeItem(ZOOM_KEY);
                        else localStorage.setItem(ZOOM_KEY, String(pageZoom));
                    } catch (e) {}
                    if (showNotice && window.__ulDlNotice) {
                        window.__ulDlNotice('info', 'Масштаб: ' + Math.round(pageZoom * 100) + '%', '');
                    }
                }
                function stepPageZoom(dir) {
                    if (dir === 0) pageZoom = 1.0;
                    else pageZoom = Math.round(Math.min(3.0, Math.max(0.3, pageZoom + dir * 0.1)) * 10) / 10;
                    applyPageZoom(true);
                }
                document.addEventListener('DOMContentLoaded', () => { if (pageZoom !== 1.0) applyPageZoom(false); });

                let keepBarUntil = 0;
                try {
                    const swTs = Number(sessionStorage.getItem('__ul_sw_bar') || '0');
                    if (swTs && (Date.now() - swTs) < 4000) {
                        keepBarUntil = Date.now() + 1100;
                    }
                    sessionStorage.removeItem('__ul_sw_bar');
                } catch (e) {}

                let tabsState = { list: [], active: 0 };
                let tabEls = [];
                let drawnCount = -1, tabClosing = false;
                let dragState = false, pendingDraw = false, dragJustEnded = false;
                const dropInd = document.createElement('div');
                dropInd.style.cssText = 'position:absolute;top:2px;width:2px;height:28px;background:#8ab4f8;border-radius:1px;display:none;pointer-events:none;';

                function optimisticSelectTab(idx) {
                    tabsState.active = idx;
                    for (let j = 0; j < tabEls.length; j++) {
                        const active = (j === idx);
                        tabEls[j].style.background = active ? '#303134' : '#171717';
                        tabEls[j].style.color = active ? '#fff' : '#9aa0a6';
                    }
                    if (ui && ui.startProgress) ui.startProgress();
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                }

                lockGlobal('__ulNavSmooth', function(url) {
                    try { if (window.__ulSaveForSwitch) window.__ulSaveForSwitch(); } catch (e) {}
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                    if (ui && ui.startProgress) ui.startProgress();
                    if (document.body) {
                        document.body.style.transition = 'opacity 0.09s ease-out';
                        document.body.style.opacity = '0.35';
                    }
                    window.location.href = url;
                });
                lockGlobal('__ulNavDirect', function(url) {
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                    if (ui && ui.startProgress) ui.startProgress();
                    window.location.href = url;
                });

                function startTabDrag(e0, from, el) {
                    const startX = e0.clientX;
                    let dragging = false, target = from;
                    const insertPos = (x) => {
                        for (let j = 0; j < tabEls.length; j++) {
                            const r = tabEls[j].getBoundingClientRect();
                            if (x < r.left + r.width / 2) return j;
                        }
                        return tabEls.length;
                    };
                    const onMove = (e) => {
                        if (!dragging) {
                            if (Math.abs(e.clientX - startX) < 6) return;
                            dragging = true; dragState = true;
                            el.style.opacity = '0.45';
                            try { el.setPointerCapture(e0.pointerId); } catch (err) {}
                        }
                        const k = insertPos(e.clientX);
                        target = k > from ? k - 1 : k;
                        const rowR = ui.tabsRow.getBoundingClientRect();
                        const edge = k < tabEls.length ? tabEls[k].getBoundingClientRect().left - 1 : tabEls[tabEls.length - 1].getBoundingClientRect().right + 1;
                        dropInd.style.left = (edge - rowR.left + ui.tabsRow.scrollLeft) + 'px';
                        dropInd.style.display = target === from ? 'none' : 'block';
                    };
                    const end = (e) => {
                        window.removeEventListener('pointermove', onMove, true);
                        window.removeEventListener('pointerup', end, true);
                        window.removeEventListener('pointercancel', end, true);
                        try { el.releasePointerCapture(e0.pointerId); } catch (err) {}
                        if (!dragging) return;
                        dragState = false;
                        dragJustEnded = true; setTimeout(() => { dragJustEnded = false; }, 80);
                        dropInd.style.display = 'none'; el.style.opacity = '';
                        if (e.type === 'pointerup' && target !== from) ipcPost('MOVE_TAB|' + from + '|' + target);
                        if (pendingDraw) { pendingDraw = false; drawTabs(); }
                    };
                    window.addEventListener('pointermove', onMove, true);
                    window.addEventListener('pointerup', end, true);
                    window.addEventListener('pointercancel', end, true);
                }

                function tabIn(el) {
                    const full = el.style.cssText;
                    el.style.cssText = full + 'min-width:0;max-width:0;padding:0;margin-right:0;opacity:0;overflow:hidden;';
                    requestAnimationFrame(() => requestAnimationFrame(() => { el.style.cssText = full; }));
                }
                function tabOut(el, done) {
                    if (tabClosing) return;
                    tabClosing = true;
                    setTimeout(() => { tabClosing = false; }, 700);
                    el.style.cssText = el.style.cssText + 'min-width:0;max-width:0;padding:0;margin-right:0;opacity:0;overflow:hidden;pointer-events:none;';
                    setTimeout(done, 170);
                }
                function drawTabs() {
                    if (!ui) return;
                    if (dragState) { pendingDraw = true; return; }
                    const row = ui.tabsRow;
                    row.replaceChildren();
                    tabEls = [];
                    tabClosing = false;
                    const prevCount = drawnCount;
                    const grew = prevCount > 0 && tabsState.list.length > prevCount;
                    drawnCount = tabsState.list.length;

                    tabsState.list.forEach((label, i) => {
                        let tabEl = document.createElement('div');
                        let isActive = (i === tabsState.active);

                        tabEl.style.cssText = `
                            min-width: 120px; max-width: 200px; height: 28px; margin-top: 2px; margin-right: 2px;
                            background: ${isActive ? '#303134' : '#171717'}; color: ${isActive ? '#fff' : '#9aa0a6'};
                            border-radius: 8px 8px 0 0; display: flex; align-items: center; justify-content: space-between;
                            padding: 0 10px; font-size: 13px; cursor: pointer; user-select: none;
                            touch-action: none; flex: none;
                        `;
                        tabEl.className = 'ul-tab';
                        tabEl.title = label || 'Новая вкладка';
                        tabEl.onmouseover = () => {
                            if (i !== tabsState.active) {
                                tabEl.style.background = '#202124';
                                ipcPost('WARM_TAB|' + i);
                            }
                        };
                        tabEl.onmouseout = () => { if (i !== tabsState.active) tabEl.style.background = '#171717'; };
                        tabEl.onclick = (e) => {
                            if (e.isTrusted && !dragJustEnded && i !== tabsState.active) {
                                optimisticSelectTab(i);
                                ipcPost('SWITCH_TAB|' + i);
                            }
                        };

                        let titleSpan = document.createElement('span');
                        titleSpan.textContent = label || 'Новая вкладка';
                        titleSpan.style.cssText = `overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-grow: 1; margin-right: 8px; pointer-events: none;`;

                        let closeBtn = document.createElement('span'); closeBtn.textContent = '×';
                        closeBtn.style.cssText = `font-size: 16px; width: 18px; height: 18px; display: flex; align-items: center; justify-content: center; border-radius: 50%; font-weight: bold; cursor: pointer; flex: none;`;
                        closeBtn.className = 'ul-tx';
                        closeBtn.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) tabOut(tabEl, () => ipcPost('CLOSE_TAB|' + i)); };

                        tabEl.addEventListener('pointerdown', (e) => {
                            if (e.isTrusted && e.button === 0 && e.target !== closeBtn) startTabDrag(e, i, tabEl);
                        });
                        tabEl.addEventListener('mousedown', (e) => { if (e.button === 1) e.preventDefault(); });
                        tabEl.addEventListener('auxclick', (e) => {
                            if (e.isTrusted && e.button === 1) { e.preventDefault(); tabOut(tabEl, () => ipcPost('CLOSE_TAB|' + i)); }
                        });

                        tabEl.appendChild(titleSpan); tabEl.appendChild(closeBtn); row.appendChild(tabEl);
                        tabEls.push(tabEl);
                        if (grew && i >= prevCount) tabIn(tabEl);
                    });

                    let newTabBtn = document.createElement('div'); newTabBtn.textContent = '+';
                    newTabBtn.style.cssText = `min-width: 28px; height: 28px; margin-top: 2px; font-size: 20px; display: flex; align-items: center; justify-content: center; cursor: pointer; border-radius: 50%; flex: none;`;
                    newTabBtn.className = 'ul-b';
                    newTabBtn.onclick = (e) => {
                        if (e.isTrusted) {
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            if (ui && ui.startProgress) ui.startProgress();
                            ipcPost('new_tab');
                        }
                    };
                    row.appendChild(newTabBtn);
                    row.appendChild(dropInd);
                }

                const warmedHosts = new Set();
                lockGlobal('__ulWarmHost', function(origin) {
                    if (!origin || warmedHosts.has(origin) || !document.head) return;
                    warmedHosts.add(origin);
                    try {
                        const l = document.createElement('link');
                        l.rel = 'preconnect';
                        l.href = origin;
                        l.crossOrigin = 'anonymous';
                        document.head.appendChild(l);
                    } catch (e) {}
                });

                function sendTitle() {
                    titleTimer = null;
                    const t = String(document.title || '').replace(/\s+/g, ' ').trim().slice(0, 120);
                    if (t === lastTitle) return;
                    lastTitle = t;
                    if (t) ipcPost('TITLE|' + t);
                }
                function scheduleTitle() { if (!titleTimer) titleTimer = setTimeout(sendTitle, 250); }
                function watchTitle() {
                    const el = document.querySelector('title');
                    if (el && el !== titleEl) {
                        titleEl = el;
                        if (titleObs) titleObs.disconnect();
                        titleObs = new MutationObserver(scheduleTitle);
                        titleObs.observe(el, { childList: true, characterData: true, subtree: true });
                    }
                    scheduleTitle();
                }
                document.addEventListener('DOMContentLoaded', watchTitle);
                window.addEventListener('load', watchTitle);
                watchTitle();

                lockGlobal('__ulRT', function(tabsString, activeIndex) {
                    if (typeof tabsString !== 'string') return;
                    tabsState = { list: tabsString.split('|||'), active: activeIndex | 0 };
                    drawTabs();
                });

                function buildUI() {
                    const startVisible = Date.now() < keepBarUntil;
                    const container = document.createElement('div'); container.id = 'light-browser-ui';
                    container.style.cssText = `position: fixed; top: ${startVisible ? '0' : '-75px'}; left: 0; width: 100%; height: 75px; background: #202124; z-index: 2147483647; font-family: sans-serif; box-shadow: 0 4px 10px rgba(0,0,0,0.5); display: flex; flex-direction: column; transition: top 0.24s cubic-bezier(0.22, 1, 0.36, 1); zoom: 1 !important;`;

                    let root = container;
                    try { root = rApply(rAttachShadow, container, [{ mode: 'closed' }]); } catch (e) { root = container; }

                    const UL_CSS = `
                        .ul-b{color:#9aa0a6;transition:background-color .15s ease,color .15s ease,transform .12s cubic-bezier(.2,.8,.3,1)}
                        .ul-b:hover{background:#303134;color:#fff}
                        .ul-b:active{background:#3c4043;transform:scale(.86)}
                        .ul-b.on{color:#fdd663}
                        .ul-b.on:hover{color:#ffe082}
                        .ul-wc{width:46px;height:32px;display:flex;align-items:center;justify-content:center;color:#9aa0a6;font-size:13px;cursor:pointer;user-select:none;transition:background-color .14s ease,color .14s ease}
                        .ul-wc:hover{background:#303134;color:#fff}
                        .ul-wc:active{background:#3c4043}
                        .ul-wc-close:hover{background:#e81123 !important;color:#fff !important}
                        .ul-wc-close:active{background:#f1707a !important;color:#fff !important}
                        .ul-pop{animation:ul-pop .42s cubic-bezier(.2,.8,.3,1)}
                        .ul-tab{transition:background-color .14s ease,color .14s ease,transform .1s ease,opacity .16s ease,min-width .16s ease,max-width .16s ease,padding .16s ease,margin .16s ease}
                        .ul-tab:active{transform:scale(.97)}
                        .ul-tx{transition:background-color .12s ease,transform .1s ease}
                        .ul-tx:hover{background:#5f6368}
                        .ul-tx:active{transform:scale(.85)}
                        .ul-x{transition:background-color .12s ease,color .12s ease,transform .1s ease}
                        .ul-x:hover{background:#3c4043;color:#fff}
                        .ul-x:active{transform:scale(.85)}
                        .ul-row{transition:background-color .12s ease,opacity .2s ease,transform .2s cubic-bezier(.4,0,.2,1),height .2s cubic-bezier(.4,0,.2,1),padding .2s ease}
                        .ul-row:hover{background:#303134}
                        .ul-row:active{background:#3c4043}
                        .ul-btn{transition:background-color .12s ease,transform .1s ease}
                        .ul-btn:hover{background:#303134}
                        .ul-btn:active{transform:scale(.95)}
                        .ul-btn2{transition:filter .12s ease,transform .1s ease}
                        .ul-btn2:hover{filter:brightness(1.12)}
                        .ul-btn2:active{transform:scale(.95)}
                        .ul-input{background:#303134;transition:background-color .15s ease,box-shadow .15s ease}
                        .ul-input:hover{background:#35363a}
                        .ul-input:focus{background:#3a3b3f;box-shadow:0 0 0 1px #8ab4f8}
                        .ul-in{animation:ul-in .17s cubic-bezier(.2,.8,.3,1) both}
                        .ul-out{animation:ul-out .12s ease-in both}
                        .ul-view{animation:ul-fade .16s ease-out both}
                        .ul-prog{position:absolute;bottom:0;left:0;height:2px;width:0;background:linear-gradient(90deg,#8ab4f8,#c58af9);opacity:0;pointer-events:none;transition:width .25s cubic-bezier(.2,.8,.3,1),opacity .2s ease}
                        @keyframes ul-pop{0%{transform:scale(1)}35%{transform:scale(1.45) rotate(-14deg)}70%{transform:scale(.92)}100%{transform:scale(1)}}
                        @keyframes ul-in{from{opacity:0;transform:translateY(-8px) scale(.97)}to{opacity:1;transform:none}}
                        @keyframes ul-out{from{opacity:1;transform:none}to{opacity:0;transform:translateY(-8px) scale(.97)}}
                        @keyframes ul-fade{from{opacity:0}to{opacity:1}}
                        @media (prefers-reduced-motion:reduce){*{animation-duration:.01ms !important;transition-duration:.01ms !important}}
                    `;
                    let ulStyled = false;
                    if (root !== container) {
                        try { const sh = new CSSStyleSheet(); sh.replaceSync(UL_CSS); root.adoptedStyleSheets = [sh]; ulStyled = true; } catch (e) {}
                    }
                    if (!ulStyled) { try { const st = document.createElement('style'); st.textContent = UL_CSS; root.appendChild(st); } catch (e) {} }

                    const topBar = document.createElement('div');
                    topBar.style.cssText = `display: flex; height: 32px; background: #000; align-items: center; width: 100%; user-select: none;`;

                    const tabsRow = document.createElement('div'); tabsRow.id = 'light-tabs-row';
                    tabsRow.style.cssText = `display: flex; flex: 1; min-width: 0; height: 32px; background: #000; overflow-x: auto; padding: 0 5px; position: relative;`;

                    tabsRow.addEventListener('mousedown', (e) => {
                        if (e.isTrusted && e.button === 0 && e.target === tabsRow) {
                            e.preventDefault();
                            ipcPost('WIN_DRAG');
                        }
                    });
                    tabsRow.addEventListener('dblclick', (e) => {
                        if (e.isTrusted && e.target === tabsRow) {
                            e.preventDefault();
                            ipcPost('WIN_MAX');
                        }
                    });

                    const winCtrls = document.createElement('div');
                    winCtrls.style.cssText = `display: flex; height: 32px; flex: none; background: #000;`;

                    const minBtn = document.createElement('div');
                    minBtn.className = 'ul-wc';
                    minBtn.textContent = '\u2500';
                    minBtn.title = 'Свернуть';
                    minBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_MIN'); };

                    const maxBtn = document.createElement('div');
                    maxBtn.className = 'ul-wc';
                    maxBtn.textContent = '\u25A1';
                    maxBtn.title = 'Развернуть / Свернуть в окно';
                    maxBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_MAX'); };

                    const closeWinBtn = document.createElement('div');
                    closeWinBtn.className = 'ul-wc ul-wc-close';
                    closeWinBtn.textContent = '\u2715';
                    closeWinBtn.title = 'Закрыть браузер';
                    closeWinBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_CLOSE'); };

                    winCtrls.appendChild(minBtn);
                    winCtrls.appendChild(maxBtn);
                    winCtrls.appendChild(closeWinBtn);

                    topBar.appendChild(tabsRow);
                    topBar.appendChild(winCtrls);

                    const urlRow = document.createElement('div');
                    urlRow.style.cssText = `display: flex; height: 43px; align-items: center; padding: 0 10px; box-sizing: border-box; position: relative;`;
                    urlRow.addEventListener('mousedown', (e) => {
                        if (e.isTrusted && e.button === 0 && e.target === urlRow) {
                            e.preventDefault();
                            ipcPost('WIN_DRAG');
                        }
                    });

                    const progBar = document.createElement('div');
                    progBar.className = 'ul-prog';
                    let progTimer = 0;
                    function startProgress() {
                        clearTimeout(progTimer);
                        progBar.style.transition = 'none';
                        progBar.style.width = '15%';
                        progBar.style.opacity = '1';
                        requestAnimationFrame(() => {
                            progBar.style.transition = 'width 0.35s cubic-bezier(.2,.8,.3,1), opacity 0.2s ease';
                            progBar.style.width = '78%';
                        });
                    }
                    function finishProgress() {
                        clearTimeout(progTimer);
                        progBar.style.width = '100%';
                        progTimer = setTimeout(() => { progBar.style.opacity = '0'; }, 140);
                    }
                    if (document.readyState !== 'complete') {
                        startProgress();
                        window.addEventListener('load', finishProgress, { once: true });
                    }

                    const trigger = document.createElement('div'); trigger.id = 'light-trigger-zone';
                    trigger.style.cssText = `position: fixed; top: 0; left: 0; width: 100%; height: 15px; z-index: 2147483646; background: transparent;`;

                    const input = document.createElement('input'); input.type = 'text'; input.value = window.location.href;
                    input.style.cssText = `flex: 1; min-width: 0; height: 30px; border-radius: 15px; border: none; padding: 0 20px; color: #fff; outline: none; font-size: 14px;`;
                    input.className = 'ul-input';
                    ['keydown', 'keyup', 'keypress'].forEach((t) => input.addEventListener(t, (ev) => ev.stopPropagation()));

                    input.addEventListener('keydown', (e) => {
                        if (e.key === 'Enter') {
                            let query = input.value.trim();
                            if (query.includes('.') && !query.includes(' ')) {
                                if (!query.startsWith('http')) query = 'https://' + query;
                                startProgress();
                                window.location.href = query;
                            } else {
                                startProgress();
                                window.location.href = 'https://duckduckgo.com/?q=' + encodeURIComponent(query);
                            }
                        }
                    });
                    input.addEventListener('focus', () => input.select());

                    const show = () => {
                        container.style.top = '0';
                        input.value = window.location.href;
                        ipcPost('GET_TABS');
                    };
                    const inputFocused = () => ((root.activeElement !== undefined ? root.activeElement : document.activeElement) === input);
                    let panelOpen = false, popOpen = false, findOpen = false;
                    const hide = () => {
                        if (dragState || panelOpen || popOpen || Date.now() < keepBarUntil) return;
                        if (!inputFocused()) container.style.top = '-75px';
                    };

                    if (startVisible) {
                        setTimeout(() => {
                            if (ui && !ui.container.matches(':hover')) hide();
                        }, 1150);
                    }

                    trigger.addEventListener('mouseenter', show);
                    container.addEventListener('mouseleave', hide);

                    function navButton(symbol, title, onClick) {
                        const b = document.createElement('div');
                        b.textContent = symbol; b.title = title;
                        b.style.cssText = `width: 30px; height: 30px; margin-right: 4px; flex: none; border-radius: 50%; font-size: 18px; display: flex; align-items: center; justify-content: center; cursor: pointer; user-select: none;`;
                        b.className = 'ul-b';
                        b.onclick = onClick;
                        return b;
                    }
                    urlRow.appendChild(navButton('\u2190', 'Назад (Alt+←)', () => { startProgress(); history.back(); }));
                    urlRow.appendChild(navButton('\u2192', 'Вперёд (Alt+→)', () => { startProgress(); history.forward(); }));
                    urlRow.appendChild(navButton('\u21BB', 'Обновить (F5)', () => { startProgress(); location.reload(); }));

                    const popBtn = navButton('\u2197', 'Вывести текущую вкладку в отдельное окно (для видеозвонков)', (e) => { if (e && e.isTrusted) ipcPost('DETACH_TAB'); });
                    popBtn.style.marginLeft = '0';

                    const US = '\u001f';
                    const starBtn = navButton('\u2606', 'Добавить страницу в закладки (Ctrl+D)', (e) => { if (e && e.isTrusted) onStar(); });
                    starBtn.style.marginLeft = '6px';
                    let starOn = false, bmTitle = '';
                    function setStar(on) {
                        starOn = !!on;
                        starBtn.className = starOn ? 'ul-b on' : 'ul-b';
                        starBtn.textContent = starOn ? '\u2605' : '\u2606';
                        starBtn.title = starOn ? 'Изменить закладку (Ctrl+D)' : 'Добавить страницу в закладки (Ctrl+D)';
                    }
                    function popStar() {
                        starBtn.className = 'ul-b on ul-pop';
                        setTimeout(() => { if (starOn) starBtn.className = 'ul-b on'; }, 430);
                    }
                    lockGlobal('__ulBM', function(on, title) {
                        if (!!on !== starOn) setStar(on);
                        bmTitle = on ? String(title || '') : '';
                    });
                    lockGlobal('__ulMaxState', function(isMax) {
                        maxBtn.textContent = isMax ? '\u2750' : '\u25A1';
                    });
                    const cleanTitle = (x) => String(x || '').replace(/\s+/g, ' ').trim().slice(0, 120);
                    const pageTitle = () => cleanTitle(document.title) || hostOf(location.href);

                    const bmPop = document.createElement('div');
                    bmPop.style.cssText = 'display:none;position:absolute;top:100%;right:78px;margin-top:6px;width:300px;max-width:calc(100vw - 16px);box-sizing:border-box;padding:14px 16px 12px;background:#202124;color:#e8eaed;font:13px/1.35 sans-serif;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.6);';
                    let popHideT = 0, popUrl = '', popOrig = '', popInput = null;
                    function closeBmPop(commit) {
                        if (!popOpen) return;
                        popOpen = false;
                        if (commit && popInput) {
                            const v = cleanTitle(popInput.value);
                            if (v && v !== popOrig) { bmTitle = v; ipcPost('BM_RENAME|' + popUrl + US + v); }
                        }
                        bmPop.className = 'ul-out';
                        clearTimeout(popHideT);
                        popHideT = setTimeout(() => { bmPop.style.display = 'none'; bmPop.textContent = ''; bmPop.className = ''; popInput = null; }, 130);
                        setTimeout(() => { if (!container.matches(':hover')) hide(); }, 0);
                    }
                    function openBmPop(isNew) {
                        if (panelOpen) closePanel();
                        clearTimeout(popHideT);
                        popOpen = true; container.style.top = '0';
                        popUrl = location.href; popOrig = bmTitle || pageTitle();
                        bmPop.textContent = '';
                        bmPop.appendChild(mk('div', 'font-size:14px;font-weight:bold;margin-bottom:10px;', isNew ? 'Закладка добавлена' : 'Изменить закладку'));
                        bmPop.appendChild(mk('div', 'font-size:11px;color:#9aa0a6;margin-bottom:4px;', 'Название'));
                        const inp = document.createElement('input'); inp.type = 'text'; inp.value = popOrig; inp.maxLength = 120;
                        inp.className = 'ul-input';
                        inp.style.cssText = 'width:100%;height:32px;box-sizing:border-box;border:none;border-radius:6px;padding:0 10px;color:#fff;outline:none;font-size:13px;';
                        ['keydown', 'keyup', 'keypress'].forEach((t) => inp.addEventListener(t, (ev) => ev.stopPropagation()));
                        inp.addEventListener('keydown', (ev) => {
                            if (ev.key === 'Enter') { ev.preventDefault(); closeBmPop(true); }
                            else if (ev.key === 'Escape') { ev.preventDefault(); closeBmPop(false); }
                        });
                        popInput = inp;
                        bmPop.appendChild(inp);
                        const btns = mk('div', 'display:flex;justify-content:flex-end;gap:8px;margin-top:12px;');
                        const delBtn = mk('div', 'padding:6px 12px;border-radius:6px;cursor:pointer;color:#f28b82;user-select:none;', 'Удалить');
                        delBtn.className = 'ul-btn';
                        delBtn.onclick = (e) => {
                            if (!e.isTrusted) return;
                            ipcPost('BM_DEL|' + popUrl); setStar(false); bmTitle = '';
                            closeBmPop(false);
                        };
                        const okBtn = mk('div', 'padding:6px 14px;border-radius:6px;cursor:pointer;background:#8ab4f8;color:#202124;font-weight:bold;user-select:none;', 'Готово');
                        okBtn.className = 'ul-btn2';
                        okBtn.onclick = (e) => { if (e.isTrusted) closeBmPop(true); };
                        btns.appendChild(delBtn); btns.appendChild(okBtn);
                        bmPop.appendChild(btns);
                        bmPop.className = 'ul-in'; bmPop.style.display = 'block';
                        inp.focus(); inp.select();
                    }
                    function onStar() {
                        if (popOpen) { closeBmPop(true); return; }
                        if (starOn) { openBmPop(false); return; }
                        bmTitle = pageTitle();
                        ipcPost('BM_ADD|' + location.href + US + bmTitle);
                        setStar(true); popStar(); openBmPop(true);
                    }

                    // ---- Умный поиск по странице (Ctrl+F) с подсчётом (1/10) и подсветкой всех слов ----
                    const findBar = document.createElement('div');
                    findBar.style.cssText = 'display:none;position:fixed;top:82px;right:14px;height:36px;padding:0 6px 0 12px;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:18px;box-shadow:0 6px 20px rgba(0,0,0,.55);align-items:center;gap:4px;z-index:2147483647;font:13px sans-serif;user-select:none;';
                    const findInp = document.createElement('input');
                    findInp.type = 'text';
                    findInp.placeholder = 'Найти на странице…';
                    findInp.style.cssText = 'width:155px;height:26px;border:none;background:transparent;color:#fff;outline:none;font-size:13px;';
                    ['keydown', 'keyup', 'keypress'].forEach((t) => findInp.addEventListener(t, (ev) => ev.stopPropagation()));

                    const findCount = document.createElement('span');
                    findCount.style.cssText = 'font-size:11px;color:#9aa0a6;min-width:38px;text-align:right;margin-right:4px;font-variant-numeric:tabular-nums;user-select:none;';
                    findCount.textContent = '';

                    let findRanges = [];
                    let findIdx = -1;
                    let lastFindQuery = '';

                    function clearFindHighlights() {
                        findRanges = [];
                        findIdx = -1;
                        lastFindQuery = '';
                        findCount.textContent = '';
                        try {
                            if (window.CSS && CSS.highlights) {
                                CSS.highlights.delete('ul-find-all');
                                CSS.highlights.delete('ul-find-cur');
                            }
                        } catch (e) {}
                    }

                    function isNodeVisible(el) {
                        while (el && el.nodeType === 1) {
                            if (el === container) return false;
                            const tag = el.tagName;
                            if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE' || tag === 'SVG' || el.hidden) return false;
                            el = el.parentElement;
                        }
                        return true;
                    }

                    function updateActiveMatch(scroll) {
                        const total = findRanges.length;
                        if (!total) {
                            findCount.textContent = findInp.value ? '0/0' : '';
                            findCount.style.color = findInp.value ? '#f28b82' : '#9aa0a6';
                            findInp.style.color = findInp.value ? '#f28b82' : '#fff';
                            try { if (window.CSS && CSS.highlights) CSS.highlights.delete('ul-find-cur'); } catch (e) {}
                            return;
                        }
                        if (findIdx < 0) findIdx = total - 1;
                        if (findIdx >= total) findIdx = 0;
                        findCount.textContent = (findIdx + 1) + '/' + total;
                        findCount.style.color = '#9aa0a6';
                        findInp.style.color = '#fff';

                        const r = findRanges[findIdx];
                        try {
                            if (window.CSS && CSS.highlights && typeof Highlight !== 'undefined') {
                                CSS.highlights.set('ul-find-cur', new Highlight(r));
                            } else {
                                const sel = window.getSelection();
                                if (sel) { sel.removeAllRanges(); sel.addRange(r); }
                            }
                            if (scroll) {
                                const el = r.startContainer && r.startContainer.parentElement;
                                if (el && el.scrollIntoView) {
                                    el.scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'nearest' });
                                }
                            }
                        } catch (e) {}
                    }

                    function buildFindMatches(q) {
                        findRanges = [];
                        findIdx = -1;
                        lastFindQuery = q;
                        if (!q || !document.body) {
                            clearFindHighlights();
                            return;
                        }
                        const needle = q.toLowerCase();
                        const nLen = needle.length;
                        const MAX_MATCHES = 2000;
                        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
                        let node;
                        while ((node = walker.nextNode())) {
                            const parent = node.parentElement;
                            if (!parent || !isNodeVisible(parent)) continue;
                            if (parent.offsetWidth === 0 && parent.offsetHeight === 0 && parent.getClientRects().length === 0) continue;
                            const val = node.nodeValue;
                            if (!val || val.length < nLen) continue;
                            const low = val.toLowerCase();
                            let pos = 0;
                            while ((pos = low.indexOf(needle, pos)) !== -1) {
                                const rng = document.createRange();
                                rng.setStart(node, pos);
                                rng.setEnd(node, pos + nLen);
                                findRanges.push(rng);
                                pos += nLen;
                                if (findRanges.length >= MAX_MATCHES) break;
                            }
                            if (findRanges.length >= MAX_MATCHES) break;
                        }

                        try {
                            if (window.CSS && CSS.highlights && typeof Highlight !== 'undefined') {
                                if (findRanges.length) {
                                    CSS.highlights.set('ul-find-all', new Highlight(...findRanges));
                                } else {
                                    CSS.highlights.delete('ul-find-all');
                                }
                            }
                        } catch (e) {}

                        if (findRanges.length) {
                            findIdx = 0;
                            for (let i = 0; i < findRanges.length; i++) {
                                const rect = findRanges[i].getBoundingClientRect();
                                if (rect && rect.top >= 0) { findIdx = i; break; }
                            }
                        }
                        updateActiveMatch(true);
                    }

                    function runFind(backwards) {
                        const q = findInp.value;
                        if (!q) { clearFindHighlights(); return; }
                        if (q !== lastFindQuery) {
                            buildFindMatches(q);
                            return;
                        }
                        if (!findRanges.length) {
                            updateActiveMatch(false);
                            return;
                        }
                        findIdx += backwards ? -1 : 1;
                        updateActiveMatch(true);
                    }

                    findInp.addEventListener('input', () => {
                        buildFindMatches(findInp.value);
                    });

                    findInp.addEventListener('keydown', (e) => {
                        if (e.key === 'Enter') { e.preventDefault(); runFind(e.shiftKey); }
                        else if (e.key === 'Escape') { e.preventDefault(); closeFind(); }
                    });

                    const mkFindBtn = (sym, title, fn) => {
                        const b = document.createElement('div');
                        b.textContent = sym; b.title = title;
                        b.style.cssText = 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;color:#9aa0a6;font-size:13px;';
                        b.className = 'ul-x';
                        b.onclick = (e) => { if (e.isTrusted) fn(); };
                        return b;
                    };
                    const findPrev = mkFindBtn('\u25B2', 'Предыдущее (Shift+Enter)', () => runFind(true));
                    const findNext = mkFindBtn('\u25BC', 'Следующее (Enter)', () => runFind(false));
                    const findClose = mkFindBtn('\u2715', 'Закрыть поиск (Esc)', () => closeFind());
                    findBar.appendChild(findInp);
                    findBar.appendChild(findCount);
                    findBar.appendChild(findPrev);
                    findBar.appendChild(findNext);
                    findBar.appendChild(findClose);

                    function openFind() {
                        findOpen = true;
                        findBar.className = 'ul-in';
                        findBar.style.display = 'flex';
                        try {
                            const sel = String(window.getSelection ? window.getSelection() : '').trim();
                            if (sel && sel.length <= 80 && !sel.includes('\n')) findInp.value = sel;
                        } catch (e) {}
                        if (findInp.value) buildFindMatches(findInp.value);
                        findInp.focus();
                        findInp.select();
                    }
                    function closeFind() {
                        if (!findOpen) return;
                        findOpen = false;
                        findBar.style.display = 'none';
                        clearFindHighlights();
                    }

                    // ---- Меню справа (три точки): История, Закладки, Загрузки, Поиск, Вернуть вкладку ----
                    const panel = document.createElement('div');
                    panel.style.cssText = 'display:none;position:absolute;top:100%;right:8px;margin-top:6px;width:380px;max-width:calc(100vw - 16px);max-height:min(70vh,520px);overflow-y:auto;background:#202124;color:#e8eaed;font:13px/1.35 sans-serif;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.6);box-sizing:border-box;';
                    let viewKind = 'menu', panelHideT = 0;
                    const mk = (tag, css, text) => {
                        const n = document.createElement(tag);
                        if (css) n.style.cssText = css;
                        if (text !== undefined) n.textContent = text;
                        return n;
                    };
                    const pad2 = (x) => (x < 10 ? '0' : '') + x;
                    const dayKey = (d) => d.getFullYear() * 10000 + d.getMonth() * 100 + d.getDate();
                    function dayLabel(ts) {
                        const d = new Date(ts * 1000), n = new Date();
                        if (dayKey(d) === dayKey(n)) return 'Сегодня';
                        if (dayKey(d) === dayKey(new Date(n.getFullYear(), n.getMonth(), n.getDate() - 1))) return 'Вчера';
                        return pad2(d.getDate()) + '.' + pad2(d.getMonth() + 1) + '.' + d.getFullYear();
                    }
                    const timeLabel = (ts) => { const d = new Date(ts * 1000); return pad2(d.getHours()) + ':' + pad2(d.getMinutes()); };
                    const hostOf = (u) => String(u).replace(/^https?:\/\//i, '').split(/[\/?#]/)[0];

                    function closePanel() {
                        if (!panelOpen) return;
                        panelOpen = false; viewKind = 'menu';
                        panel.className = 'ul-out';
                        clearTimeout(panelHideT);
                        panelHideT = setTimeout(() => { panel.style.display = 'none'; panel.textContent = ''; panel.className = ''; }, 130);
                        setTimeout(() => { if (!container.matches(':hover')) hide(); }, 0);
                    }
                    function panelHeader(title, extra) {
                        const h = mk('div', 'display:flex;align-items:center;padding:8px 8px;border-bottom:1px solid #3c4043;position:sticky;top:0;background:#202124;z-index:1;');
                        const back = mk('div', 'width:28px;height:28px;display:flex;align-items:center;justify-content:center;border-radius:50%;cursor:pointer;color:#9aa0a6;font-size:16px;', '\u2190');
                        back.title = 'Назад в меню';
                        back.className = 'ul-x';
                        back.onclick = (e) => { if (e.isTrusted) showMenu(); };
                        h.appendChild(back);
                        h.appendChild(mk('div', 'flex:1;font-weight:bold;margin-left:8px;', title));
                        if (extra) h.appendChild(extra);
                        return h;
                    }
                    function showMenu() {
                        viewKind = 'menu';
                        panel.textContent = '';
                        const box = mk('div', 'padding:6px 0;');
                        box.className = 'ul-view';
                        [
                            ['История (Ctrl+H)', 'HIST_LIST'],
                            ['Закладки (Ctrl+D)', 'BM_LIST'],
                            ['Загрузки (Ctrl+J)', 'DL_LIST'],
                        ].forEach((it) => {
                            const r = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', it[0]);
                            r.className = 'ul-row';
                            r.onclick = (e) => { if (!e.isTrusted) return; openLoading(it[0].split(' (')[0]); ipcPost(it[1]); };
                            box.appendChild(r);
                        });
                        const findRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', 'Найти на странице (Ctrl+F)');
                        findRow.className = 'ul-row';
                        findRow.onclick = (e) => { if (!e.isTrusted) return; closePanel(); openFind(); };
                        box.appendChild(findRow);

                        const reopenRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', 'Вернуть закрытую вкладку (Ctrl+Shift+T)');
                        reopenRow.className = 'ul-row';
                        reopenRow.onclick = (e) => {
                            if (!e.isTrusted) return;
                            closePanel();
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            startProgress();
                            ipcPost('REOPEN_TAB');
                        };
                        box.appendChild(reopenRow);

                        // Быстрое управление масштабом страницы прямо из меню
                        const zoomRow = mk('div', 'display:flex;align-items:center;justify-content:space-between;padding:8px 16px;border-top:1px solid #3c4043;margin-top:4px;user-select:none;');
                        zoomRow.appendChild(mk('span', 'color:#9aa0a6;', 'Масштаб страницы'));
                        const zCtrls = mk('div', 'display:flex;align-items:center;gap:6px;');
                        const zMinus = mk('div', 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#fff;font-weight:bold;', '\u2212');
                        zMinus.className = 'ul-btn';
                        const zVal = mk('span', 'min-width:44px;text-align:center;font-variant-numeric:tabular-nums;cursor:pointer;', Math.round(pageZoom * 100) + '%');
                        zVal.title = 'Сбросить на 100% (Ctrl+0)';
                        const zPlus = mk('div', 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#fff;font-weight:bold;', '+');
                        zPlus.className = 'ul-btn';
                        zMinus.onclick = (e) => { if (e.isTrusted) { stepPageZoom(-1); zVal.textContent = Math.round(pageZoom * 100) + '%'; } };
                        zPlus.onclick = (e) => { if (e.isTrusted) { stepPageZoom(1); zVal.textContent = Math.round(pageZoom * 100) + '%'; } };
                        zVal.onclick = (e) => { if (e.isTrusted) { stepPageZoom(0); zVal.textContent = '100%'; } };
                        zCtrls.appendChild(zMinus); zCtrls.appendChild(zVal); zCtrls.appendChild(zPlus);
                        zoomRow.appendChild(zCtrls);
                        box.appendChild(zoomRow);

                        panel.appendChild(box);
                    }
                    function openLoading(title) {
                        viewKind = 'loading';
                        panel.textContent = '';
                        const v = mk('div'); v.className = 'ul-view';
                        v.appendChild(panelHeader(title));
                        panel.appendChild(v);
                    }
                    function openPanel() {
                        if (popOpen) closeBmPop(true);
                        clearTimeout(panelHideT);
                        panelOpen = true; container.style.top = '0';
                        showMenu();
                        panel.className = 'ul-in'; panel.style.display = 'block';
                    }
                    function openUrl(u, background) {
                        if (!/^https?:\/\//i.test(u)) return;
                        if (background) ipcPost('OPEN_TAB_BG|' + u);
                        else { closePanel(); startProgress(); location.href = u; }
                    }
                    function collapseRow(row, done) {
                        row.style.height = row.getBoundingClientRect().height + 'px';
                        row.style.overflow = 'hidden'; row.style.pointerEvents = 'none';
                        void row.offsetHeight;
                        requestAnimationFrame(() => {
                            row.style.height = '0'; row.style.paddingTop = '0'; row.style.paddingBottom = '0';
                            row.style.opacity = '0'; row.style.transform = 'translateX(28px)';
                        });
                        setTimeout(done, 210);
                    }
                    function listRow(kind, it) {
                        const isDl = kind === 'downloads';
                        const row = mk('div', 'display:flex;align-items:center;padding:6px 8px 6px 16px;cursor:pointer;box-sizing:border-box;');
                        row.className = 'ul-row';
                        row.title = isDl ? 'Показать в папке: ' + String(it.u) : String(it.u);
                        const col = mk('div', 'flex:1;min-width:0;');
                        col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;', String(it.t || it.u)));
                        const sub = isDl
                            ? timeLabel(it.d) + '  \u00B7  ' + String(it.u)
                            : hostOf(it.u) + (kind === 'history' ? '  \u00B7  ' + timeLabel(it.d) : '');
                        col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:11px;', sub));
                        row.appendChild(col);

                        if (isDl) {
                            const showBtn = mk('div', 'width:26px;height:26px;flex:none;margin-left:6px;border-radius:50%;display:flex;align-items:center;justify-content:center;color:#8ab4f8;font-size:14px;', '\uD83D\uDCC2');
                            showBtn.className = 'ul-x';
                            showBtn.title = 'Показать файл в папке';
                            showBtn.onclick = (e) => {
                                e.stopPropagation();
                                if (e.isTrusted) ipcPost('DL_SHOW|' + it.u);
                            };
                            row.appendChild(showBtn);
                        }

                        const del = mk('div', 'width:24px;height:24px;flex:none;margin-left:4px;border-radius:50%;display:flex;align-items:center;justify-content:center;color:#9aa0a6;font-size:13px;', '\u2715');
                        del.className = 'ul-x';
                        del.title = isDl ? 'Убрать из списка' : (kind === 'history' ? 'Удалить из истории' : 'Удалить закладку');
                        del.onclick = (e) => {
                            e.stopPropagation();
                            if (!e.isTrusted) return;
                            const cmd = (isDl ? 'DL_DEL|' : (kind === 'history' ? 'HIST_DEL|' : 'BM_DEL|')) + it.u;
                            collapseRow(row, () => ipcPost(cmd));
                        };
                        if (isDl) {
                            row.onclick = (e) => { if (e.isTrusted) ipcPost('DL_SHOW|' + it.u); };
                        } else {
                            row.onclick = (e) => { if (e.isTrusted) openUrl(String(it.u), e.ctrlKey || e.metaKey); };
                            row.onauxclick = (e) => { if (e.isTrusted && e.button === 1) { e.preventDefault(); openUrl(String(it.u), true); } };
                        }
                        row.appendChild(del);
                        return row;
                    }
                    function showList(kind, items) {
                        const fresh = viewKind !== kind;
                        const keep = panel.scrollTop;
                        viewKind = kind;
                        panel.textContent = '';
                        const body = mk('div');
                        if (fresh) body.className = 'ul-view';
                        let extra = null;
                        if (kind === 'history') {
                            extra = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', 'Очистить');
                            extra.className = 'ul-btn';
                            let armed = false;
                            extra.onclick = (e) => {
                                if (!e.isTrusted) return;
                                if (!armed) {
                                    armed = true; extra.textContent = 'Удалить всё?'; extra.style.color = '#f28b82';
                                    setTimeout(() => { armed = false; extra.textContent = 'Очистить'; extra.style.color = '#8ab4f8'; }, 3000);
                                    return;
                                }
                                body.style.transition = 'opacity .2s ease, transform .2s ease';
                                body.style.opacity = '0'; body.style.transform = 'translateY(-8px)';
                                setTimeout(() => ipcPost('HIST_CLEAR'), 220);
                            };
                        } else if (kind === 'downloads') {
                            extra = mk('div', 'display:flex;align-items:center;gap:4px;');
                            const folderBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', '\uD83D\uDCC2 Папка');
                            folderBtn.className = 'ul-btn';
                            folderBtn.title = 'Открыть папку Загрузки в Проводнике';
                            folderBtn.onclick = (e) => { if (e.isTrusted) ipcPost('DL_FOLDER'); };
                            extra.appendChild(folderBtn);
                            if (items.length) {
                                const clrBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#9aa0a6;font-size:12px;user-select:none;', 'Очистить');
                                clrBtn.className = 'ul-btn';
                                let armed = false;
                                clrBtn.onclick = (e) => {
                                    if (!e.isTrusted) return;
                                    if (!armed) {
                                        armed = true; clrBtn.textContent = 'Точно?'; clrBtn.style.color = '#f28b82';
                                        setTimeout(() => { armed = false; clrBtn.textContent = 'Очистить'; clrBtn.style.color = '#9aa0a6'; }, 3000);
                                        return;
                                    }
                                    body.style.transition = 'opacity .2s ease, transform .2s ease';
                                    body.style.opacity = '0'; body.style.transform = 'translateY(-8px)';
                                    setTimeout(() => ipcPost('DL_CLEAR'), 220);
                                };
                                extra.appendChild(clrBtn);
                            }
                        }
                        const headerTitle = kind === 'history' ? 'История' : (kind === 'bookmarks' ? 'Закладки' : 'Загрузки');
                        panel.appendChild(panelHeader(headerTitle, extra));
                        panel.appendChild(body);
                        if (!items.length) {
                            const emptyText = kind === 'history'
                                ? 'История пуста'
                                : (kind === 'bookmarks' ? 'Закладок пока нет. Нажмите \u2606 в адресной строке.' : 'Список загрузок пуст');
                            const empty = mk('div', 'padding:18px 16px;color:#9aa0a6;', emptyText);
                            empty.className = 'ul-view';
                            body.appendChild(empty);
                            return;
                        }
                        let lastDay = '';
                        items.forEach((it) => {
                            if (!it || typeof it.u !== 'string') return;
                            if (kind === 'history' || kind === 'downloads') {
                                const day = dayLabel(it.d);
                                if (day !== lastDay) {
                                    lastDay = day;
                                    body.appendChild(mk('div', 'padding:10px 16px 2px;color:#9aa0a6;font-size:11px;text-transform:uppercase;letter-spacing:.04em;', day));
                                }
                            }
                            body.appendChild(listRow(kind, it));
                        });
                        if (!fresh) panel.scrollTop = keep;
                    }
                    lockGlobal('__ulPanel', function(kind, json) {
                        if (!panelOpen) return;
                        let items = [];
                        try { items = rApply(rJsonParse, JSON, [String(json)]); } catch (e) { items = []; }
                        const k = kind === 'bookmarks' ? 'bookmarks' : (kind === 'downloads' ? 'downloads' : 'history');
                        showList(k, Array.isArray(items) ? items : []);
                    });
                    const menuBtn = navButton('\u22EF', 'Меню', (e) => { if (e && e.isTrusted) { if (panelOpen) closePanel(); else openPanel(); } });
                    menuBtn.style.marginRight = '0';
                    root.addEventListener('pointerdown', (e) => {
                        if (!e.isTrusted) return;
                        const t = e.target;
                        if (popOpen && !bmPop.contains(t) && t !== starBtn) closeBmPop(true);
                        if (panelOpen && !panel.contains(t) && t !== menuBtn) closePanel();
                    }, true);
                    window.addEventListener('pointerdown', (e) => {
                        if (!(panelOpen || popOpen) || !e.isTrusted) return;
                        if (e.target === container || container.contains(e.target)) return;
                        if (popOpen) closeBmPop(true);
                        if (panelOpen) closePanel();
                    }, true);
                    root.appendChild(panel); root.appendChild(bmPop); root.appendChild(findBar);

                    urlRow.appendChild(input); urlRow.appendChild(starBtn); urlRow.appendChild(popBtn); urlRow.appendChild(menuBtn); urlRow.appendChild(progBar);
                    root.appendChild(topBar); root.appendChild(urlRow);

                    ui = {
                        container, trigger, tabsRow, input, show, hide, inputFocused, startProgress, finishProgress,
                        openFind, findStep: runFind, isFindOpen: () => findOpen,
                        closeOverlays: () => {
                            const any = popOpen || panelOpen || findOpen;
                            if (popOpen) closeBmPop(false);
                            if (panelOpen) closePanel();
                            if (findOpen) closeFind();
                            return any;
                        },
                        toggleBookmark: onStar,
                        openHistory: () => { if (!panelOpen) openPanel(); openLoading('История'); ipcPost('HIST_LIST'); },
                        openDownloads: () => { if (!panelOpen) openPanel(); openLoading('Загрузки'); ipcPost('DL_LIST'); },
                    };
                    lockGlobal('__ulFlash', function() {
                        if (!ui) return;
                        ui.container.style.top = '0';
                        setTimeout(() => { if (ui && !ui.container.matches(':hover')) ui.hide(); }, 2000);
                    });
                    drawTabs();
                    ipcPost('GET_TABS');
                }

                // ---- Ручное скрытие элементов: Ctrl+Shift+X (выбрать), Ctrl+Shift+Z (отменить последнее) ----
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
                    t.style.cssText = 'position:fixed;left:16px;bottom:16px;max-width:60%;padding:10px 14px;background:#202124;color:#fff;font:13px sans-serif;border-radius:8px;z-index:2147483647;box-shadow:0 2px 10px rgba(0,0,0,.5);pointer-events:none;opacity:0;transform:translateY(8px);transition:opacity .2s ease,transform .2s cubic-bezier(.2,.8,.3,1);zoom:1 !important;';
                    document.documentElement.appendChild(t);
                    requestAnimationFrame(() => requestAnimationFrame(() => { t.style.opacity = '1'; t.style.transform = 'none'; }));
                    setTimeout(() => { t.style.opacity = '0'; t.style.transform = 'translateY(8px)'; }, 3250);
                    setTimeout(() => t.remove(), 3500);
                }

                // ---- Компактная плашка-пилюля загрузки и масштаба в правом нижнем углу ----
                let dlChip = null, dlTimer = 0;
                lockGlobal('__ulDlNotice', function(state, name, fullPath) {
                    if (!document.documentElement) return;
                    clearTimeout(dlTimer);
                    if (!dlChip || !dlChip.isConnected) {
                        dlChip = document.createElement('div');
                        dlChip.style.cssText = 'position:fixed;right:14px;bottom:14px;max-width:270px;height:32px;padding:0 12px;background:rgba(32,33,36,.95);color:#e8eaed;border:1px solid #3c4043;border-radius:16px;z-index:2147483647;box-shadow:0 4px 12px rgba(0,0,0,.45);display:flex;align-items:center;gap:7px;font:12px/1 "Segoe UI",system-ui,sans-serif;user-select:none;opacity:0;transform:translateY(6px);transition:opacity .18s ease,transform .18s cubic-bezier(.2,.8,.3,1),border-color .15s ease;zoom:1 !important;';
                        document.documentElement.appendChild(dlChip);
                    }
                    dlChip.replaceChildren();
                    const icon = document.createElement('span');
                    icon.style.cssText = 'font-size:13px;flex:none;font-weight:bold;';
                    const txt = document.createElement('span');
                    txt.style.cssText = 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;min-width:0;';
                    txt.textContent = String(name || '');

                    if (state === 'start') {
                        icon.textContent = '\u2B07';
                        icon.style.color = '#8ab4f8';
                        dlChip.style.cursor = 'default';
                        dlChip.title = 'Скачивается: ' + name;
                        dlChip.onclick = null;
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    } else if (state === 'ok') {
                        icon.textContent = '\u2714';
                        icon.style.color = '#81c995';
                        dlChip.style.cursor = 'pointer';
                        dlChip.title = 'Нажмите, чтобы показать в папке';
                        const openIcon = document.createElement('span');
                        openIcon.textContent = '\uD83D\uDCC2';
                        openIcon.style.cssText = 'font-size:12px;flex:none;opacity:.85;margin-left:2px;';
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                        dlChip.appendChild(openIcon);
                        dlChip.onmouseenter = () => { dlChip.style.borderColor = '#8ab4f8'; };
                        dlChip.onmouseleave = () => { dlChip.style.borderColor = '#3c4043'; };
                        dlChip.onclick = (e) => {
                            if (!e.isTrusted) return;
                            if (fullPath) ipcPost('DL_SHOW|' + fullPath);
                            else ipcPost('DL_FOLDER');
                            dlChip.style.opacity = '0';
                            dlChip.style.transform = 'translateY(6px)';
                            setTimeout(() => { if (dlChip) { dlChip.remove(); dlChip = null; } }, 180);
                        };
                    } else if (state === 'info') {
                        icon.textContent = '\uD83D\uDD0D';
                        icon.style.color = '#8ab4f8';
                        dlChip.style.cursor = 'pointer';
                        dlChip.title = 'Нажмите, чтобы сбросить масштаб (100%)';
                        dlChip.onclick = (e) => { if (e.isTrusted) stepPageZoom(0); };
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    } else {
                        icon.textContent = '\u2716';
                        icon.style.color = '#f28b82';
                        dlChip.style.cursor = 'default';
                        dlChip.title = 'Ошибка загрузки';
                        dlChip.onclick = null;
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    }
                    requestAnimationFrame(() => {
                        if (dlChip) { dlChip.style.opacity = '1'; dlChip.style.transform = 'none'; }
                    });
                    const delay = state === 'start' ? 12000 : (state === 'info' ? 1600 : 3800);
                    dlTimer = setTimeout(() => {
                        if (!dlChip) return;
                        dlChip.style.opacity = '0';
                        dlChip.style.transform = 'translateY(6px)';
                        setTimeout(() => { if (dlChip) { dlChip.remove(); dlChip = null; } }, 200);
                    }, delay);
                });

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
                    PICK_BLOCKED.forEach((t) => window.addEventListener(t, swallow, true));
                    window.removeEventListener('keydown', onPickKey, true);
                    if (hl) { hl.remove(); hl = null; }
                }
                function undoRule() {
                    const s = userRules.pop();
                    saveRules(); ensureUserCss();
                    toast(s ? 'Правило удалено: ' + s : 'Для этого сайта нет сохранённых правил');
                }

                // ---- Сохранение прокрутки, позиции видео и текста в полях ввода ----
                const STATE_KEY = 'ultralight_tabstate';
                const STATE_TTL = 12 * 60 * 60 * 1000;
                const RESTART_TTL = 7 * 24 * 60 * 60 * 1000;
                const STATE_MAX = 60;
                const RUN_ID = '__RUN_ID__';
                const RESTORE_URLS = __RESTORE_URLS__;
                const FORM_MAX = 20, FORM_LEN = 2000;
                let scrollDone = true, videoDone = true, formDone = true;
                let switching = false;
                let scrollTimer = null;

                function stateKey(h) {
                    try {
                        const u = new URL(h || window.location.href);
                        if (/(^|\.)youtube\.com$/i.test(u.hostname) && u.pathname === '/watch') return u.origin + '/watch?v=' + (u.searchParams.get('v') || '');
                        return u.origin + u.pathname + u.search;
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
                        if (ui && ui.container.contains(el)) continue;
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
                            if (!el) return;
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
                        if (!Object.keys(pending).length || ticks >= 40) stop();
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
                    INPUT_EVENTS.forEach((ev) => window.addEventListener(ev, stop, { capture: true, passive: true }));
                    const step = () => {
                        if (stopped) return;
                        ticks++;
                        const root = document.documentElement;
                        const h = Math.max(root ? root.scrollHeight : 0, document.body ? document.body.scrollHeight : 0);
                        const maxY = h - window.innerHeight;
                        if (maxY >= y - 2) {
                            window.scrollTo({ left: x, top: y, behavior: 'instant' });
                            if (Math.abs(window.scrollY - y) <= 3) stable++; else stable = 0;
                            if (stable >= 2) stop();
                        } else if (maxY > 0 && ticks <= 12) {
                            window.scrollTo({ left: x, top: maxY, behavior: 'instant' });
                        }
                        if (ticks >= 30) stop();
                    };
                    document.addEventListener('DOMContentLoaded', step, { once: true });
                    timer = setInterval(step, 120);
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
                    document.addEventListener('loadedmetadata', onMeta, true);
                }

                (function restoreOnLoad() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const states = readStates();
                    const key = stateKey();
                    const st = states[key];
                    if (!st) return;
                    const age = Date.now() - (st.ts || 0);
                    const bySwitch = !!st.sw && age <= STATE_TTL;
                    const byRestart = RESTORE_KEYS.has(key) && st.run !== RUN_ID && age <= RESTART_TTL;
                    if (!bySwitch && !byRestart) return;
                    st.sw = false; st.run = RUN_ID; writeStates(states);
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
                window.addEventListener('pageshow', (e) => {
                    if (e.persisted && document.body) {
                        document.body.style.opacity = '1';
                        if (ui && ui.finishProgress) ui.finishProgress();
                    }
                });
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
                lockGlobal('__ulSaveForSwitch', () => { switching = true; saveState(); });

                // Масштабирование ТОЛЬКО страницы колесом мыши с зажатым Ctrl
                let ctrlDown = false;
                window.addEventListener('keydown', (e) => { if (e.key === 'Control') ctrlDown = true; }, true);
                window.addEventListener('keyup', (e) => { if (e.key === 'Control') ctrlDown = false; }, true);
                window.addEventListener('blur', () => { ctrlDown = false; });

                let lastWheelZoom = 0;
                const onCtrlWheel = (e) => {
                    if (!e.ctrlKey && !e.metaKey && !ctrlDown) return;
                    e.preventDefault();
                    e.stopPropagation();
                    e.stopImmediatePropagation();
                    const now = perfNow();
                    if (now - lastWheelZoom < 35) return;
                    lastWheelZoom = now;
                    const dy = e.deltaY !== undefined ? e.deltaY : -(e.wheelDelta || 0);
                    if (dy === 0) return;
                    stepPageZoom(dy < 0 ? 1 : -1);
                };
                window.addEventListener('wheel', onCtrlWheel, { passive: false, capture: true });
                document.addEventListener('wheel', onCtrlWheel, { passive: false, capture: true });
                window.addEventListener('mousewheel', onCtrlWheel, { passive: false, capture: true });

                window.addEventListener('keydown', (e) => {
                    if (!ui || !e.isTrusted) return;
                    if (e.key === 'Escape' && ui.closeOverlays()) { e.preventDefault(); return; }
                    if (e.key === 'F11') { e.preventDefault(); ipcPost('WIN_FULLSCREEN'); return; }
                    if (e.key === 'F3') {
                        e.preventDefault();
                        if (!ui.isFindOpen()) ui.openFind();
                        else ui.findStep(e.shiftKey);
                        return;
                    }
                    const k = e.key.toLowerCase();
                    const code = e.code || '';
                    if (e.altKey && !e.ctrlKey) {
                        if (e.key === 'ArrowLeft') { e.preventDefault(); ui.startProgress(); history.back(); }
                        else if (e.key === 'ArrowRight') { e.preventDefault(); ui.startProgress(); history.forward(); }
                        return;
                    }
                    if (e.key === 'F5' || (e.ctrlKey && !e.shiftKey && (k === 'r' || code === 'KeyR'))) {
                        e.preventDefault(); ui.startProgress(); location.reload(); return;
                    }
                    if (e.ctrlKey && e.shiftKey && !e.altKey) {
                        if (k === 'x' || code === 'KeyX') { e.preventDefault(); startPick(); return; }
                        if (k === 'z' || code === 'KeyZ') { e.preventDefault(); undoRule(); return; }
                        if (k === 't' || code === 'KeyT') {
                            e.preventDefault();
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            ui.startProgress();
                            ipcPost('REOPEN_TAB');
                            return;
                        }
                    }
                    if (!e.ctrlKey || e.altKey) return;
                    // Масштаб только страницы: Ctrl + (+ / - / 0)
                    if (e.key === '+' || e.key === '=' || code === 'Equal' || code === 'NumpadAdd') {
                        e.preventDefault(); stepPageZoom(1); return;
                    }
                    if (e.key === '-' || e.key === '_' || code === 'Minus' || code === 'NumpadSubtract') {
                        e.preventDefault(); stepPageZoom(-1); return;
                    }
                    if (e.key === '0' || code === 'Digit0' || code === 'Numpad0') {
                        e.preventDefault(); stepPageZoom(0); return;
                    }
                    if ((k === 'f' || code === 'KeyF') && !e.shiftKey) {
                        e.preventDefault(); ui.openFind(); return;
                    }
                    if (k === 'l' || code === 'KeyL') { e.preventDefault(); ui.show(); ui.input.focus(); }
                    else if ((k === 't' || code === 'KeyT') && !e.shiftKey) {
                        e.preventDefault();
                        try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                        ui.startProgress();
                        ipcPost('new_tab');
                    }
                    else if (k === 'w' || code === 'KeyW') { e.preventDefault(); ipcPost('CLOSE_TAB|-1'); }
                    else if (k === 'tab') {
                        e.preventDefault();
                        if (tabsState.list.length > 1) {
                            optimisticSelectTab((tabsState.active + 1) % tabsState.list.length);
                        }
                        ipcPost('next_tab');
                    }
                    else if ((k === 'd' || code === 'KeyD') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.toggleBookmark(); }
                    else if ((k === 'h' || code === 'KeyH') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.openHistory(); }
                    else if ((k === 'j' || code === 'KeyJ') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.openDownloads(); }
                }, true);

                function attachUI() {
                    const root = document.documentElement;
                    if (!root) return;
                    if (!ui) buildUI();
                    if (!ui.trigger.isConnected) root.appendChild(ui.trigger);
                    if (!ui.container.isConnected) root.appendChild(ui.container);
                    if (pageZoom !== 1.0 && document.body && !document.body.style.zoom) {
                        document.body.style.zoom = String(pageZoom);
                    }
                }

                let observedRoot = null, observedHead = null, docObserved = false;
                function keepAlive() {
                    if (!ui || !ui.container.isConnected || !ui.trigger.isConnected) attachUI();
                    injectCss();
                    applyCosmetic();
                    ensureUserCss();
                    observeHead();
                    watchTitle();
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

                // ---- Контекстное меню (правая кнопка) ----
                let ctxMenu = null;
                function closeCtx() {
                    if (!ctxMenu) return;
                    const old = ctxMenu; ctxMenu = null;
                    old.style.pointerEvents = 'none'; old.style.opacity = '0';
                    setTimeout(() => old.remove(), 120);
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
                function triggerDownload(u) {
                    try {
                        const a = document.createElement('a');
                        a.href = u;
                        a.setAttribute('download', '');
                        (document.body || document.documentElement).appendChild(a);
                        origElemClick.call(a);
                        a.remove();
                    } catch (e) {}
                }

                function showCtx(e) {
                    closeCtx();
                    const t = e.target;
                    const items = [];
                    const sep = () => { if (items.length && items[items.length - 1] !== null) items.push(null); };
                    const isHttp = (u) => /^https?:\/\//i.test(u);

                    // Ссылка
                    const link = t.closest ? t.closest('a[href], area[href]') : null;
                    if (link) {
                        let abs = '';
                        try { abs = new URL(typeof link.href === 'string' ? link.href : link.getAttribute('href'), window.location.href).href; } catch (err) {}
                        if (abs) {
                            if (isHttp(abs) || abs.startsWith('blob:') || abs.startsWith('data:')) {
                                if (isHttp(abs)) {
                                    items.push({ label: 'Открыть ссылку в новой вкладке', fn: () => openInTab(abs, true) });
                                    items.push({ label: 'Открыть ссылку в новом окне', fn: () => openInWindow(abs) });
                                }
                                items.push({ label: 'Скачать по ссылке', fn: () => triggerDownload(abs) });
                            }
                            items.push({ label: 'Копировать адрес ссылки', fn: () => copyText(abs) });
                            sep();
                        }
                    }
                    // Изображение
                    const img = t.closest ? t.closest('img') : null;
                    const src = img ? (img.currentSrc || img.src || '') : '';
                    if (src && (isHttp(src) || src.startsWith('blob:') || src.startsWith('data:'))) {
                        if (isHttp(src)) {
                            items.push({ label: 'Открыть изображение в новой вкладке', fn: () => openInTab(src, true) });
                        }
                        items.push({ label: 'Скачать изображение', fn: () => triggerDownload(src) });
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
                    // Видео: картинка в картинке
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
                    items.push({ label: 'Найти на странице… (Ctrl+F)', fn: () => { if (ui) ui.openFind(); } });
                    items.push({ label: 'Вернуть закрытую вкладку (Ctrl+Shift+T)', fn: () => ipcPost('REOPEN_TAB') });
                    sep();
                    items.push({ label: 'Печать…', fn: () => window.print() });
                    items.push({ label: 'Выделить всё', fn: () => { try { window.getSelection().selectAllChildren(document.body || document.documentElement); } catch (err) {} } });
                    items.push({ label: 'Копировать адрес страницы', fn: () => copyText(window.location.href) });
                    items.push({ label: 'Вывести текущую вкладку в отдельное окно', fn: () => ipcPost('DETACH_TAB') });
                    while (items.length && items[items.length - 1] === null) items.pop();

                    // Прикрепляем контекстное меню к documentElement (<html>), чтобы зум body его не увеличивал
                    const m = document.createElement('div');
                    m.style.cssText = 'position:fixed;left:0;top:0;z-index:2147483647;min-width:230px;max-width:360px;padding:6px 0;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.5);font:13px/1.4 "Segoe UI",system-ui,sans-serif;user-select:none;-webkit-user-select:none;zoom:1 !important;';
                    m.addEventListener('mousedown', (ev) => ev.preventDefault());
                    m.addEventListener('contextmenu', (ev) => ev.preventDefault());
                    m.style.transformOrigin = 'top left'; m.style.opacity = '0'; m.style.transform = 'scale(.96)';
                    m.style.transition = 'opacity .12s ease, transform .12s cubic-bezier(.2,.8,.3,1)';
                    items.forEach((it) => {
                        const row = document.createElement('div');
                        if (it === null) {
                            row.style.cssText = 'height:1px;margin:6px 0;background:#3c4043;';
                        } else {
                            row.textContent = it.label;
                            row.style.cssText = 'padding:6px 18px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;transition:background-color .12s ease;';
                            row.onmouseover = () => { row.style.background = '#303134'; };
                            row.onmouseout = () => { row.style.background = 'transparent'; };
                            row.onclick = (ev) => { closeCtx(); if (!ev.isTrusted) return; try { it.fn(); } catch (err) {} };
                        }
                        m.appendChild(row);
                    });
                    document.documentElement.appendChild(m);
                    const w = m.offsetWidth, h = m.offsetHeight;
                    m.style.left = Math.max(4, Math.min(e.clientX, window.innerWidth - w - 4)) + 'px';
                    m.style.top = Math.max(4, Math.min(e.clientY, window.innerHeight - h - 4)) + 'px';
                    ctxMenu = m;
                    requestAnimationFrame(() => requestAnimationFrame(() => { m.style.opacity = '1'; m.style.transform = 'none'; }));
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
                    if (ui && ui.container.contains(t)) { e.preventDefault(); return; }
                    if (t.isContentEditable || (t.closest && t.closest('input, textarea, select'))) return;
                    e.preventDefault();
                    showCtx(e);
                }, false);

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

    let cosmetic_script = format!(
        "if (window === window.top) {{ window.__ulCosmRaw = function () {{ return '{}'; }}; }}",
        js_str(&cosmetic_js)
    );

    let profile_path = app_dir.join("User Data");
    let mut web_context = WebContext::new(Some(profile_path));

    let proxy_waiter = proxy.clone();
    let proxy_newwin = proxy.clone();
    let proxy_dl_start = proxy.clone();
    let proxy_dl_done = proxy.clone();
    let last_dl_target = std::sync::Arc::new(std::sync::Mutex::new(PathBuf::new()));
    let last_dl_for_start = last_dl_target.clone();
    let last_dl_for_done = last_dl_target;

    // Отключаем встроенный хоткей-зум WebView2 (.with_hotkeys_zoom(false)), чтобы он не увеличивал панель браузера
    let builder = WebViewBuilder::new(&window)
        .with_background_color((32, 33, 36, 255))
        .with_hotkeys_zoom(false)
        .with_autoplay(true)
        .with_devtools(false)
        .with_web_context(&mut web_context)
        .with_initialization_script(&cosmetic_script)
        .with_initialization_script(&script)
        .with_ipc_handler(move |msg: String| {
            if msg.len() > MAX_IPC_LEN {
                return;
            }
            if let Some(rest) = msg.strip_prefix(ipc_prefix.as_str()) {
                let _ = proxy.send_event(rest.to_string());
            }
        })
        .with_navigation_handler(|url: String| nav_allowed(&url))
        .with_new_window_req_handler(move |url: String| {
            let _ = proxy_newwin.send_event(format!("NEWWIN|{}", url));
            false
        })
        .with_download_started_handler(move |url: String, path: &mut PathBuf| {
            if url.starts_with("file:") {
                return false;
            }
            let dest = unique_download_path(path);
            let name = dest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("файл")
                .to_string();
            if let Ok(mut guard) = last_dl_for_start.lock() {
                *guard = dest.clone();
            }
            *path = dest;
            let _ = proxy_dl_start.send_event(format!("DL_START|{}", name));
            true
        })
        .with_download_completed_handler(move |_url, path, success| {
            let resolved = path.or_else(|| {
                last_dl_for_done
                    .lock()
                    .ok()
                    .map(|g| g.clone())
                    .filter(|p| !p.as_os_str().is_empty())
            });
            let full_path = resolved
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = resolved
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("файл")
                .to_string();
            let status = if success { "1" } else { "0" };
            let _ = proxy_dl_done.send_event(format!("DL_DONE|{}|{}|{}", status, name, full_path));
        })
        .with_url(&start_url)
        .unwrap();

    let mut builder = builder;
    for s in &user_scripts {
        builder = builder.with_initialization_script(s);
    }

    #[cfg(windows)]
    let builder = {
        let rules = dns_rules;
        let mut disabled = String::from(
            "msWebOOUI,msPdfOOUI,msMiniMenu,Translate,AutofillServerCommunication,CertificateTransparencyComponentUpdater"
        );
        if !SMARTSCREEN {
            disabled.push_str(",msSmartScreenProtection");
        }
        let webrtc = if WEBRTC_HIDE_LOCAL_IP {
            " --force-webrtc-ip-handling-policy=default_public_interface_only"
        } else {
            ""
        };
        builder.with_additional_browser_args(format!(
            "--disable-features={} --enable-features=CanvasOopRasterization,VaapiVideoDecoder,BackForwardCache,SmoothScrolling,ViewTransitionOnNavigation --enable-smooth-scrolling --disable-pinch --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --autoplay-policy=no-user-gesture-required{} --host-resolver-rules=\"{}\"",
            disabled, webrtc, rules
        ))
    };

    let webview = builder.build().unwrap();
    // Сбрасываем возможный старый системный зум WebView2 в 100%, чтобы панель вкладок была стандартного размера
    let _ = webview.zoom(1.0);

    #[cfg(windows)]
    {
        if MEDIA_MODE != 0 {
            let trusted = load_trusted_media(&trusted_media_path);
            if let Err(e) = media_permissions::install(&webview, trusted) {
                let _ = fs::write(app_dir.join("media-permissions.log"), format!("{:?}", e));
            }
        }
    }
    drop(script);
    drop(cosmetic_script);
    drop(user_scripts);

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
            Err(e) => {
                let _ = fs::write(app_dir.join("native-adblock.log"), format!("{:?}", e));
            }
        }
    }

    let mut tabs: Vec<String> = tabs_init;
    let mut current_tab = current_init;
    let mut closed_tabs: Vec<(String, usize)> = Vec::new();
    let mut last_saved = String::new();
    let mut titles: HashMap<String, String> = HashMap::new();
    let history_path = app_dir.join("history.txt");
    let bookmarks_path = app_dir.join("bookmarks.txt");
    let downloads_path = app_dir.join("downloads.txt");
    compact_history(&history_path);
    let mut hist = HistState::new();
    let mut bm_map: HashMap<String, String> = read_entries(&bookmarks_path).into_iter().map(|e| (e.url, e.title)).collect();
    if let Some(sp) = &save_path {
        save_session(sp, &mut last_saved, &tabs, current_tab);
    }
    let mut popout_alive: u32 = 0;
    let mut popout_seq: u32 = 0;
    let mut switch_at: Option<std::time::Instant> = None;
    let mut perf_lines: u32 = 0;
    let mut rl_start = std::time::Instant::now();
    let mut rl_count: u32 = 0;
    let mut last_spawn: Option<std::time::Instant> = None;
    let mut last_newwin: Option<std::time::Instant> = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event: WindowEvent::Moved(pos), .. } => {
                let is_max = window.is_maximized() || window.fullscreen().is_some();
                if !is_max && !window.is_minimized() && pos.x > -10000 && pos.y > -10000 {
                    win_state.x = pos.x;
                    win_state.y = pos.y;
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                let is_max = window.is_maximized() || window.fullscreen().is_some();
                win_state.max = is_max;
                if !is_max && !window.is_minimized() && size.width >= 200 && size.height >= 150 {
                    win_state.w = size.width;
                    win_state.h = size.height;
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                hist.flush(&history_path);
                if let Some(sp) = &save_path {
                    save_session(sp, &mut last_saved, &tabs, current_tab);
                }
                if session_enabled {
                    if !window.is_minimized() {
                        win_state.max = window.is_maximized() || window.fullscreen().is_some();
                    }
                    save_win_state(&win_state_path, &win_state);
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(msg) => {
                if !msg.starts_with("SYNC_URL|")
                    && !msg.starts_with("POPOUT_CLOSED|")
                    && !msg.starts_with("DL_START|")
                    && !msg.starts_with("DL_DONE|")
                {
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
                if let Some(u) = msg.strip_prefix("SYNC_URL|") {
                    if !is_http_url(u) {
                        return;
                    }
                    hist.leave(&history_path, u);
                }
                if PERF_LOG
                    && (msg.starts_with("SWITCH_TAB|") || msg == "next_tab" || msg == "new_tab" || msg.starts_with("OPEN_TAB|"))
                {
                    switch_at = Some(std::time::Instant::now());
                }
                if msg.starts_with("SYNC_URL|") {
                    let url = msg["SYNC_URL|".len()..].to_string();
                    if PERF_LOG {
                        if let Some(t) = switch_at.take() {
                            perf_log(&app_dir, &format!("switch->page started: {} ms  ({})", t.elapsed().as_millis(), url.split('/').nth(2).unwrap_or("")));
                        }
                    }
                    #[cfg(all(windows, feature = "native-adblock"))]
                    {
                        *page_url.borrow_mut() = url.clone();
                    }
                    let label = titles.get(&url).cloned().unwrap_or_else(|| tab_label(&url));
                    window.set_title(&format!("{} — Falco", label));
                    tabs[current_tab] = url;
                } else if msg == "WIN_MIN" {
                    window.set_minimized(true);
                } else if msg == "WIN_MAX" {
                    if window.fullscreen().is_some() {
                        window.set_fullscreen(None);
                    } else {
                        let next = !window.is_maximized();
                        window.set_maximized(next);
                        win_state.max = next;
                        let _ = webview.evaluate_script(&format!(
                            "if(window.__ulMaxState) window.__ulMaxState({});",
                            if next { "true" } else { "false" }
                        ));
                    }
                } else if msg == "WIN_FULLSCREEN" {
                    if window.fullscreen().is_some() {
                        window.set_fullscreen(None);
                    } else {
                        window.set_fullscreen(Some(tao::window::Fullscreen::Borderless(None)));
                    }
                } else if msg == "WIN_CLOSE" {
                    hist.flush(&history_path);
                    if let Some(sp) = &save_path {
                        save_session(sp, &mut last_saved, &tabs, current_tab);
                    }
                    if session_enabled {
                        if !window.is_minimized() {
                            win_state.max = window.is_maximized() || window.fullscreen().is_some();
                        }
                        save_win_state(&win_state_path, &win_state);
                    }
                    *control_flow = ControlFlow::Exit;
                } else if msg == "WIN_DRAG" {
                    let _ = window.drag_window();
                } else if msg == "REOPEN_TAB" {
                    if let Some((url, old_idx)) = closed_tabs.pop() {
                        if tabs.len() < MAX_TABS {
                            let pos = old_idx.min(tabs.len());
                            tabs.insert(pos, url.clone());
                            current_tab = pos;
                            let label = titles.get(&url).cloned().unwrap_or_else(|| tab_label(&url));
                            window.set_title(&format!("{} — Falco", label));
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!(
                                "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                                js_str(&tabs_str),
                                current_tab,
                                nav_script(&url, true)
                            );
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if let Some(idx_str) = msg.strip_prefix("WARM_TAB|") {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        if let Some(u) = tabs.get(idx) {
                            if let Some(h) = host_of(u) {
                                let scheme = if u.starts_with("http://") { "http://" } else { "https://" };
                                let origin = format!("{}{}", scheme, h);
                                let _ = webview.evaluate_script(&format!(
                                    "if(window.__ulWarmHost) window.__ulWarmHost('{}');",
                                    js_str(&origin)
                                ));
                            }
                        }
                    }
                } else if msg == "GET_TABS" {
                    let tabs_str = tabs_labels(&tabs, &titles);
                    let is_max = window.is_maximized() || window.fullscreen().is_some();
                    let js = format!(
                        "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulMaxState) window.__ulMaxState({}); {}",
                        js_str(&tabs_str),
                        current_tab,
                        if is_max { "true" } else { "false" },
                        bm_state_js(&bm_map, tabs[current_tab].as_str())
                    );
                    let _ = webview.evaluate_script(&js);
                } else if msg == "new_tab" && tabs.len() < MAX_TABS {
                    tabs.push("https://duckduckgo.com".to_string());
                    current_tab = tabs.len() - 1;
                    let tabs_str = tabs_labels(&tabs, &titles);
                    let js = format!(
                        "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                        js_str(&tabs_str),
                        current_tab,
                        nav_script("https://duckduckgo.com", true)
                    );
                    let _ = webview.evaluate_script(&js);
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB|") {
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        current_tab = tabs.len() - 1;
                        let tabs_str = tabs_labels(&tabs, &titles);
                        let js = format!(
                            "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                            js_str(&tabs_str),
                            current_tab,
                            nav_script(url, true)
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(url) = msg.strip_prefix("OPEN_WINDOW|") {
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
                    if is_http_url(url) && tabs.len() < MAX_TABS && spawn_ok(&mut last_newwin) {
                        tabs.push(url.to_string());
                        let tabs_str = tabs_labels(&tabs, &titles);
                        let js = format!(
                            "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulFlash) window.__ulFlash();",
                            js_str(&tabs_str),
                            current_tab
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB_BG|") {
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        let tabs_str = tabs_labels(&tabs, &titles);
                        let js = format!(
                            "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulFlash) window.__ulFlash();",
                            js_str(&tabs_str),
                            current_tab
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(name) = msg.strip_prefix("DL_START|") {
                    let js = format!("if(window.__ulDlNotice) window.__ulDlNotice('start','{}','');", js_str(name));
                    let _ = webview.evaluate_script(&js);
                } else if let Some(rest) = msg.strip_prefix("DL_DONE|") {
                    let mut it = rest.splitn(3, '|');
                    let ok = it.next().unwrap_or("0");
                    let name = it.next().unwrap_or("файл");
                    let full_path = it.next().unwrap_or("");
                    if !full_path.is_empty() {
                        let status_title = if ok == "1" {
                            format!("✔ {}", name)
                        } else {
                            format!("✖ {} (ошибка)", name)
                        };
                        dl_add(&downloads_path, full_path, &status_title);
                    }
                    let state = if ok == "1" { "ok" } else { "err" };
                    let list = dl_view(&downloads_path);
                    let js = format!(
                        "if(window.__ulDlNotice) window.__ulDlNotice('{}','{}','{}'); {}",
                        state,
                        js_str(name),
                        js_str(full_path),
                        panel_js("downloads", &list)
                    );
                    let _ = webview.evaluate_script(&js);
                } else if msg == "DL_LIST" {
                    let list = dl_view(&downloads_path);
                    let _ = webview.evaluate_script(&panel_js("downloads", &list));
                } else if msg == "DL_FOLDER" {
                    open_downloads_folder();
                } else if let Some(fp) = msg.strip_prefix("DL_SHOW|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        show_in_folder(fp);
                    }
                } else if let Some(fp) = msg.strip_prefix("DL_DEL|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        dl_del(&downloads_path, fp);
                        let list = dl_view(&downloads_path);
                        let _ = webview.evaluate_script(&panel_js("downloads", &list));
                    }
                } else if msg == "DL_CLEAR" {
                    let _ = fs::remove_file(&downloads_path);
                    let _ = webview.evaluate_script(&panel_js("downloads", &[]));
                } else if let Some(rest) = msg.strip_prefix("PERF|") {
                    if PERF_LOG && perf_lines < 500 {
                        perf_lines += 1;
                        let clean: String = rest.chars().filter(|c| !c.is_control()).take(200).collect();
                        perf_log(&app_dir, &format!("page: init|ttfb|dcl|load|host = {}", clean));
                    }
                } else if msg == "HIST_LIST" {
                    hist.flush(&history_path);
                    let list = history_view(&history_path, HISTORY_SHOW);
                    let _ = webview.evaluate_script(&panel_js("history", &list));
                } else if let Some(u) = msg.strip_prefix("HIST_DEL|") {
                    if is_http_url(u) {
                        hist.flush(&history_path);
                        history_del(&history_path, u);
                        hist.last = None;
                        let list = history_view(&history_path, HISTORY_SHOW);
                        let _ = webview.evaluate_script(&panel_js("history", &list));
                    }
                } else if msg == "HIST_CLEAR" {
                    hist.pending = None;
                    hist.last = None;
                    let _ = fs::remove_file(&history_path);
                    let _ = webview.evaluate_script(&panel_js("history", &[]));
                } else if msg == "BM_LIST" {
                    let list = bm_view(&bookmarks_path);
                    bm_map = list.iter().map(|e| (e.url.clone(), e.title.clone())).collect();
                    let _ = webview.evaluate_script(&panel_js("bookmarks", &list));
                } else if let Some(rest) = msg.strip_prefix("BM_ADD|") {
                    if let Some((u, t)) = rest.split_once(FIELD_SEP) {
                        if is_http_url(u) {
                            let title = bm_add(&bookmarks_path, u, &clean_title(t));
                            bm_map.insert(u.to_string(), title);
                            let _ = webview.evaluate_script(&bm_state_js(&bm_map, u));
                        }
                    }
                } else if let Some(rest) = msg.strip_prefix("BM_RENAME|") {
                    if let Some((u, t)) = rest.split_once(FIELD_SEP) {
                        let title = clean_title(t);
                        if is_http_url(u) && !title.is_empty() && bm_rename(&bookmarks_path, u, &title) {
                            bm_map.insert(u.to_string(), title);
                        }
                    }
                } else if let Some(u) = msg.strip_prefix("BM_DEL|") {
                    if is_http_url(u) {
                        bm_del(&bookmarks_path, u);
                        bm_map.remove(u);
                        let list = bm_view(&bookmarks_path);
                        let js = format!("{} {}", panel_js("bookmarks", &list), bm_state_js(&bm_map, tabs[current_tab].as_str()));
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(t) = msg.strip_prefix("TITLE|") {
                    let t: String = t.chars().filter(|c| !c.is_control()).take(120).collect::<String>().replace("|||", "|");
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        let key = tabs[current_tab].clone();
                        hist.note(&history_path, &key, &t);
                        if titles.get(&key) != Some(&t) {
                            if titles.len() >= 200 {
                                titles.retain(|k, _| tabs.contains(k));
                            }
                            titles.insert(key, t.clone());
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                        window.set_title(&format!("{} — Falco", t));
                    }
                } else if let Some(rest) = msg.strip_prefix("MOVE_TAB|") {
                    let mut it = rest.split('|');
                    let from = it.next().and_then(|x| x.parse::<usize>().ok());
                    let to = it.next().and_then(|x| x.parse::<usize>().ok());
                    if let (Some(from), Some(to)) = (from, to) {
                        if from < tabs.len() && to < tabs.len() && from != to {
                            let moved = tabs.remove(from);
                            tabs.insert(to, moved);
                            current_tab = if current_tab == from {
                                to
                            } else {
                                let mut c = current_tab;
                                if from < c { c -= 1; }
                                if to <= c { c += 1; }
                                c
                            };
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg.starts_with("SWITCH_TAB|") {
                    if let Ok(idx) = msg.replace("SWITCH_TAB|", "").parse::<usize>() {
                        if idx < tabs.len() && idx != current_tab {
                            current_tab = idx;
                            let target_url = &tabs[current_tab];
                            let label = titles.get(target_url).cloned().unwrap_or_else(|| tab_label(target_url));
                            window.set_title(&format!("{} — Falco", label));
                            let _ = webview.evaluate_script(&nav_script(target_url, true));
                        }
                    }
                } else if msg.starts_with("CLOSE_TAB|") {
                    let mut idx_to_close = current_tab;
                    let parsed = msg.replace("CLOSE_TAB|", "").parse::<i32>().unwrap_or(-1);
                    if parsed >= 0 { idx_to_close = parsed as usize; }
                    
                    if tabs.len() > 1 && idx_to_close < tabs.len() {
                        let was_active = idx_to_close == current_tab;
                        let closed_url = tabs.remove(idx_to_close);
                        if closed_tabs.len() >= MAX_CLOSED_TABS {
                            closed_tabs.remove(0);
                        }
                        closed_tabs.push((closed_url, idx_to_close));
                        
                        if current_tab > idx_to_close { current_tab -= 1; } 
                        else if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                        
                        if was_active {
                            let target_url = &tabs[current_tab];
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!(
                                "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                                js_str(&tabs_str),
                                current_tab,
                                nav_script(target_url, false)
                            );
                            let _ = webview.evaluate_script(&js);
                        } else {
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    } else if tabs.len() <= 1 {
                        hist.flush(&history_path);
                        if session_enabled {
                            if !window.is_minimized() {
                                win_state.max = window.is_maximized() || window.fullscreen().is_some();
                            }
                            save_win_state(&win_state_path, &win_state);
                        }
                        *control_flow = ControlFlow::Exit;
                    }
                } else if msg == "DETACH_TAB" {
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
                                tabs[0] = DEFAULT_URL.to_string();
                                current_tab = 0;
                                let _ = webview.evaluate_script(&nav_script(DEFAULT_URL, false));
                            }
                        }
                    }
                } else if let Some(id) = msg.strip_prefix("POPOUT_CLOSED|") {
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
                            let tabs_str = tabs_labels(&tabs, &titles);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg == "next_tab" {
                    if tabs.len() > 1 {
                        current_tab = (current_tab + 1) % tabs.len();
                        let target_url = &tabs[current_tab];
                        let label = titles.get(target_url).cloned().unwrap_or_else(|| tab_label(target_url));
                        window.set_title(&format!("{} — Falco", label));
                        let _ = webview.evaluate_script(&nav_script(target_url, true));
                    }
                }
                if msg != "GET_TABS" && !msg.starts_with("WARM_TAB|") {
                    if let Some(sp) = &save_path {
                        save_session(sp, &mut last_saved, &tabs, current_tab);
                    }
                }
            }
            _ => {}
        }
    });
}
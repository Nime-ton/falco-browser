//! Нативный блокировщик на движке `adblock` (feature "native-adblock", только Windows).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::time::Duration;

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

fn build_engine(dir: &Path) -> (Engine, usize) {
    let mut lines: Vec<String> = Vec::new();
    for path in crate::util::list_files(dir, "txt") {
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

/// Запрос к движку: (url, страница, тип ресурса). Ответ - (номер запроса, "блокировать?").
/// Если `seq == 0`, воркер пересобирает движок из обновлённых файлов на диске.
struct Query {
    seq: u64,
    url: String,
    page: String,
    rtype: &'static str,
    reply: mpsc::Sender<(u64, bool)>,
}

/// Связь с фоновым потоком, в котором живёт движок.
/// Сам `Engine` между потоками не передаётся (он создаётся и используется только в воркере),
/// поэтому от него не требуется `Send`. По каналу ходят только строки и bool.
#[derive(Clone)]
pub struct Checker {
    tx: mpsc::Sender<Query>,
    ready: Arc<AtomicBool>,
}

impl Checker {
    /// Пересобирает нативный движок после скачивания обновлённых списков фильтров.
    pub fn reload(&self) {
        let (dummy_tx, _) = mpsc::channel();
        let _ = self.tx.send(Query {
            seq: 0,
            url: String::new(),
            page: String::new(),
            rtype: "other",
            reply: dummy_tx,
        });
    }
}

/// Запускает поток, который собирает движок (разбор ~140 тыс. правил занимает время) и затем
/// отвечает на запросы. Пока движок не готов, `install` пропускает запросы без проверки
/// (сетевую блокировку по хостам и косметику это не затрагивает - они работают сразу).
pub fn spawn_engine(dir: &Path, log_path: PathBuf) -> Checker {
    let (tx, rx) = mpsc::channel::<Query>();
    let ready = Arc::new(AtomicBool::new(false));
    let ready_w = ready.clone();
    let dir = dir.to_path_buf();
    let _ = std::thread::Builder::new().name("adblock".into()).spawn(move || {
        if DEBUG {
            let _ = std::fs::write(&log_path, "");
        }
        let (mut engine, n) = build_engine(&dir);
        if DEBUG {
            log(&log_path, &format!("filter lines loaded: {}\n", n));
        }
        ready_w.store(true, Ordering::Release);
        let mut logged = 0u32;
        for q in rx {
            if q.seq == 0 {
                let (new_engine, count) = build_engine(&dir);
                engine = new_engine;
                if DEBUG {
                    log(&log_path, &format!("filter lines reloaded: {}\n", count));
                }
                continue;
            }
            let (matched, bad) = match Request::new(&q.url, &q.page, q.rtype) {
                Ok(req) => (engine.check_network_request(&req).matched, false),
                Err(_) => (false, true),
            };
            if DEBUG && logged < 400 {
                logged += 1;
                log(
                    &log_path,
                    &format!("{} {} page={} matched={} bad_request={}\n", q.rtype, q.url, q.page, matched, bad),
                );
            }
            let _ = q.reply.send((q.seq, matched));
        }
    });
    Checker { tx, ready }
}

fn skip_page(page: &str, whitelist: &Arc<RwLock<HashSet<String>>>) -> bool {
    const SKIP: &[&str] = &[
        "youtube.com",
        "youtube-nocookie.com",
        "gemini.google.com",
        "aistudio.google.com",
        "notebooklm.google.com",
        "docs.google.com",
        "drive.google.com",
        "mail.google.com",
        "chatgpt.com",
        "openai.com",
        "claude.ai",
        "deepseek.com",
        "grok.com",
        "x.ai",
        "perplexity.ai",
        "copilot.microsoft.com",
        "poe.com",
        "mistral.ai",
        "huggingface.co",
        "github.com",
        "gitlab.com",
    ];
    match crate::util::host_of(page) {
        Some(h) => {
            if crate::util::is_under_any(&h, SKIP) {
                return true;
            }
            if let Ok(wl) = whitelist.read() {
                if crate::filters::is_whitelisted_host(&h, &wl) {
                    return true;
                }
            }
            false
        }
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
    checker: Checker,
    page_url: Rc<RefCell<String>>,
    whitelist: Arc<RwLock<HashSet<String>>>,
    _log_path: PathBuf,
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

        // один постоянный канал ответов; номер запроса защищает от запоздалого ответа на прошлый запрос
        let (reply_tx, reply_rx) = mpsc::channel::<(u64, bool)>();
        let mut seq = 0u64;
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
            if skip_page(&page, &whitelist) {
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
            // движок ещё собирается -> пропускаем без проверки
            let matched = if checker.ready.load(Ordering::Acquire) {
                seq += 1;
                if seq == 0 {
                    seq = 1;
                }
                let q = Query { seq, url, page, rtype, reply: reply_tx.clone() };
                if checker.tx.send(q).is_ok() {
                    loop {
                        match reply_rx.recv_timeout(Duration::from_millis(250)) {
                            Ok((s, m)) if s == seq => break m,
                            Ok(_) => continue,
                            Err(_) => break false,
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            };
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
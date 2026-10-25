//! Нативный блокировщик на движке `adblock` (feature "native-adblock", только Windows).

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

fn skip_page(page: &str) -> bool {
    match crate::util::host_of(page) {
        Some(h) => crate::util::is_under(&h, "youtube.com") || crate::util::is_under(&h, "youtube-nocookie.com"),
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
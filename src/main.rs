#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Falco - лёгкий браузер на WebView2 (wry + tao).

mod config;
mod util;
mod filters;
mod userscripts;
mod media;
mod tabs;
mod history;
mod downloads;
mod session;
mod suggest;
#[cfg(windows)]
mod popups;
#[cfg(all(windows, feature = "native-adblock"))]
mod native_adblock;

use std::collections::HashMap;
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

use crate::config::*;
use crate::util::*;
use crate::filters::*;
use crate::userscripts::*;
use crate::media::*;
use crate::tabs::*;
use crate::history::*;
use crate::downloads::*;
use crate::session::*;
use crate::suggest::{suggest_js, SUGGEST_MAX};

/// Основной скрипт браузера (вкладки, панель, блокировка). Подставляются __IPC_TOKEN__, __AD_HOSTS__ и др.
const BROWSER_JS: &str = include_str!("../js/browser.js");

/// Фатальная ошибка при старте: пишем startup-error.log и (на Windows) показываем окно.
/// Без этого при windows_subsystem = "windows" и panic = "abort" программа просто исчезает.
fn fatal(app_dir: &Path, what: &str, err: &str) -> ! {
    let log = app_dir.join("startup-error.log");
    let _ = fs::create_dir_all(app_dir);
    let _ = fs::write(&log, format!("{}\n{}\n", what, err));
    #[cfg(windows)]
    unsafe {
        use windows::core::{w, HSTRING};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let text = HSTRING::from(format!(
            "{}\n\n{}\n\nЕсли ошибка про WebView2 - установите Microsoft Edge WebView2 Runtime (evergreen) с сайта Microsoft.\nПодробности: {}",
            what,
            err,
            log.display()
        ));
        MessageBoxW(HWND::default(), &text, w!("Falco"), MB_OK | MB_ICONERROR);
    }
    std::process::exit(1);
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

    let _ = fs::create_dir_all(&app_dir);

    // panic = "abort" убивает процесс сразу - хотя бы оставим причину в crash.log
    {
        let crash_log = app_dir.join("crash.log");
        std::panic::set_hook(Box::new(move |info| {
            if let Some(dir) = crash_log.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&crash_log, format!("{}\n", info));
        }));
    }

    let settings_path = app_dir.join("settings.txt");
    let mut settings = Settings::load(&settings_path);
    let dl_dir_shared = std::sync::Arc::new(std::sync::Mutex::new(settings.download_dir.clone()));
    #[cfg(windows)]
    let media_mode_shared = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(settings.media_mode));

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

    let window = wb
        .build(&event_loop)
        .unwrap_or_else(|e| fatal(&app_dir, "Не удалось создать окно", &format!("{:?}", e)));

    if session_enabled && win_state.max {
        window.set_maximized(true);
    }

    let filters_dir = app_dir.join("filters");
    let scripts_dir = app_dir.join("scripts");
    let _ = fs::create_dir_all(&filters_dir);
    let _ = fs::create_dir_all(&scripts_dir);

    // Автоматически создаем базовый файл фильтров в AppData при первом запуске,
    // чтобы скомпилированный .exe сразу работал с блокировщиком у любого пользователя!
    let default_filter_file = filters_dir.join("default-block.txt");
    if !default_filter_file.exists() {
        let embedded_filters = 
            "||doubleclick.net^\n\
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
        let _ = fs::write(&default_filter_file, embedded_filters);
    }
    
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
    let (tabs_init, current_init, pinned_init) = match cli_url {
        Some(u) => (vec![u], 0, 0),
        None if settings.restore_session => {
            let (t, a, p) = load_session(&session_path);
            if t.is_empty() {
                (vec![settings.start_url.clone()], 0, 0)
            } else {
                restored = true;
                (t, a, p)
            }
        }
        None => (vec![settings.start_url.clone()], 0, 0),
    };
    let start_url = tabs_init[current_init].clone();

    let ret_id: Option<String> = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--return=").map(|s| s.to_string()))
        .filter(|s| !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_digit() || b == b'_'));
    let save_path: Option<PathBuf> = if session_enabled {
        Some(session_path.clone())
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

    let script = BROWSER_JS
        .replace("__RUN_ID__", &run_id)
        .replace("__RESTORE_URLS__", &restore_urls_js)
        .replace("__PERF__", if PERF_LOG { "true" } else { "false" })
        .replace("__IPC_TOKEN__", &ipc_secret)
        .replace("__NATIVE__", if cfg!(all(windows, feature = "native-adblock")) { "true" } else { "false" })
        .replace("__AD_HOSTS__", &ad_hosts_js)
        .replace("__SEARCH_URL__", settings.search_url_prefix())
        .replace("__YT_H264__", if settings.yt_force_h264 { "true" } else { "false" })
        .replace("__PIN_BAR__", if settings.pin_top_bar { "true" } else { "false" });

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
    let dl_dir_for_handler = dl_dir_shared.clone();

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
        .with_download_started_handler(move |url: String, path: &mut PathBuf| {
            if url.starts_with("file:") {
                return false;
            }
            let custom = dl_dir_for_handler.lock().ok().map(|g| g.clone()).unwrap_or_default();
            let dest = unique_download_path(path, &custom);
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
        .unwrap_or_else(|e| fatal(&app_dir, "Не удалось подготовить WebView", &format!("{:?}", e)));

    let mut builder = builder;
    for s in &user_scripts {
        builder = builder.with_initialization_script(s);
    }

    // На Windows новые окна обрабатывает popups::install (попапы входа/оплаты + вкладки для target=_blank).
    #[cfg(not(windows))]
    let builder = {
        let p = proxy_newwin.clone();
        builder.with_new_window_req_handler(move |url: String| {
            let _ = p.send_event(format!("NEWWIN|{}", url));
            false
        })
    };

    #[cfg(windows)]
    let builder = {
        let rules = dns_rules;
        let mut disabled = String::from(
            "msWebOOUI,msPdfOOUI,msMiniMenu,Translate,AutofillServerCommunication,CertificateTransparencyComponentUpdater"
        );
        if !settings.smartscreen {
            disabled.push_str(",msSmartScreenProtection");
        }
        let webrtc = if settings.webrtc_hide_ip {
            " --force-webrtc-ip-handling-policy=default_public_interface_only"
        } else {
            ""
        };
        builder.with_additional_browser_args(format!(
            "--disable-features={} --enable-features=CanvasOopRasterization,VaapiVideoDecoder,BackForwardCache,SmoothScrolling,ViewTransitionOnNavigation --enable-smooth-scrolling --disable-pinch --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --autoplay-policy=no-user-gesture-required{} --host-resolver-rules=\"{}\"",
            disabled, webrtc, rules
        ))
    };

    let webview = builder
        .build()
        .unwrap_or_else(|e| fatal(&app_dir, "Не удалось запустить WebView2", &format!("{:?}", e)));
    // Сбрасываем возможный старый системный зум WebView2 в 100%, чтобы панель вкладок была стандартного размера
    let _ = webview.zoom(1.0);

    #[cfg(windows)]
    {
        let trusted = load_trusted_media(&trusted_media_path);
        if let Err(e) = media::permissions::install(&webview, trusted, media_mode_shared.clone()) {
            let _ = fs::write(app_dir.join("media-permissions.log"), format!("{:?}", e));
        }
    }
    #[cfg(windows)]
    let popups_state = popups::new_state();
    #[cfg(windows)]
    {
        if let Err(e) = popups::install(&webview, popups_state.clone(), proxy_newwin.clone()) {
            let _ = fs::write(app_dir.join("popups.log"), format!("{:?}", e));
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
    let mut pinned: usize = pinned_init; // сколько вкладок закреплено (они всегда первые)
    let mut closed_tabs: Vec<(String, usize, bool)> = Vec::new(); // (url, индекс, была закреплена)
    let mut blank_tab = false; // true: единственная вкладка - пустая стартовая, оставшаяся после закрытия последней
    let mut last_saved = String::new();
    let mut titles: HashMap<String, String> = HashMap::new();
    let history_path = app_dir.join("history.txt");
    let bookmarks_path = app_dir.join("bookmarks.txt");
    let downloads_path = app_dir.join("downloads.txt");
    compact_history(&history_path);
    let mut hist = HistState::new(settings.history_enabled);
    let mut bm_map: HashMap<String, String> = read_entries(&bookmarks_path).into_iter().map(|e| (e.url, e.title)).collect();
    if let Some(sp) = &save_path {
        if !session_enabled || settings.restore_session {
            save_session(sp, &mut last_saved, &tabs, current_tab, pinned);
        }
    }
    let mut popout_alive: u32 = 0;
    let mut popout_seq: u32 = 0;
    let mut switch_at: Option<std::time::Instant> = None;
    let mut perf_lines: u32 = 0;
    let mut rl_start = std::time::Instant::now();
    let mut rl_count: u32 = 0;
    let mut last_spawn: Option<std::time::Instant> = None;
    let mut last_newwin: Option<std::time::Instant> = None;
    let mut cur_doc: Option<String> = None;

    event_loop.run(move |event, elwt, control_flow| {
        *control_flow = ControlFlow::Wait;
        #[cfg(not(windows))]
        let _ = elwt;

        match event {
            // события попапов (закрытие, размер, фокус) - не путать с главным окном
            Event::WindowEvent { window_id, event: wev, .. } if window_id != window.id() => {
                #[cfg(windows)]
                popups::on_window_event(&popups_state, window_id, &wev);
                #[cfg(not(windows))]
                let _ = (window_id, wev);
            }
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
                    if !session_enabled || settings.restore_session {
                        save_session(sp, &mut last_saved, &tabs, current_tab, pinned);
                    } else {
                        let _ = fs::remove_file(sp);
                    }
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
                let msg = match filter_doc_msg(&msg, &mut cur_doc) {
                    Some(m) => m,
                    None => return,
                };
                if !msg.starts_with("SYNC_URL|")
                    && !msg.starts_with("POPOUT_CLOSED|")
                    && !msg.starts_with("DL_START|")
                    && !msg.starts_with("DL_DONE|")
                    && msg != "POPUP_REQ"
                    && !msg.starts_with("POPUP_CLOSE|")
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
                } else if msg == "POPUP_REQ" {
                    #[cfg(windows)]
                    {
                        popups::open_pending(elwt, &window, &popups_state, &proxy_waiter);
                    }
                } else if let Some(id) = msg.strip_prefix("POPUP_CLOSE|") {
                    #[cfg(windows)]
                    {
                        if let Ok(id) = id.parse::<u32>() {
                            popups::close_id(&popups_state, id);
                        }
                    }
                    #[cfg(not(windows))]
                    let _ = id;
                } else if let Some(rest) = msg.strip_prefix("SUGGEST|") {
                    // "SUGGEST|<номер запроса>|<текст>": подсказки адресной строки из истории и закладок
                    if let Some((seq, q)) = rest.split_once('|') {
                        if !seq.is_empty() && seq.len() <= 9 && seq.bytes().all(|b| b.is_ascii_digit()) && q.len() <= 400 {
                            let list = crate::suggest::suggest(&history_path, &bookmarks_path, q, SUGGEST_MAX);
                            let _ = webview.evaluate_script(&suggest_js(seq, &list));
                        }
                    }
                } else if msg == "SETTINGS_GET" {
                    let js = format!(
                        "if(window.__ulSettings) window.__ulSettings('{}');",
                        js_str(&settings.to_json())
                    );
                    let _ = webview.evaluate_script(&js);
                } else if msg == "PICK_DL_DIR" {
                    if let Some(folder) = pick_download_folder(&window) {
                        let js = format!(
                            "if(window.__ulPickedDlDir) window.__ulPickedDlDir('{}');",
                            js_str(&folder)
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                } else if let Some(rest) = msg.strip_prefix("SETTINGS_SAVE|") {
                    for part in rest.split(FIELD_SEP) {
                        if let Some((k, v)) = part.split_once('=') {
                            match k {
                                "start_url" if is_http_url(v) => settings.start_url = v.to_string(),
                                "search_engine" if matches!(v, "ddg" | "google" | "yandex" | "bing" | "brave") => {
                                    settings.search_engine = v.to_string();
                                }
                                "restore_session" => settings.restore_session = v == "1",
                                "download_dir" if v.len() <= 500 && !v.chars().any(|c| c.is_control()) => {
                                    settings.download_dir = v.trim().to_string();
                                    if let Ok(mut g) = dl_dir_shared.lock() {
                                        *g = settings.download_dir.clone();
                                    }
                                }
                                "history_enabled" => {
                                    settings.history_enabled = v == "1";
                                    hist.enabled = settings.history_enabled;
                                }
                                "media_mode" => {
                                    if let Ok(m) = v.parse::<u8>() {
                                        settings.media_mode = m.min(2);
                                        #[cfg(windows)]
                                        media_mode_shared.store(settings.media_mode, std::sync::atomic::Ordering::Relaxed);
                                    }
                                }
                                "smartscreen" => settings.smartscreen = v == "1",
                                "webrtc_hide_ip" => settings.webrtc_hide_ip = v == "1",
                                "yt_force_h264" => settings.yt_force_h264 = v == "1",
                                "pin_top_bar" => settings.pin_top_bar = v == "1",
                                _ => {}
                            }
                        }
                    }
                    settings.save(&settings_path);
                    if !settings.restore_session {
                        let _ = fs::remove_file(&session_path);
                    }
                    let js = format!(
                        "if(window.__ulSettings) window.__ulSettings('{}'); if(window.__ulDlNotice) window.__ulDlNotice('ok','Настройки сохранены','');",
                        js_str(&settings.to_json())
                    );
                    let _ = webview.evaluate_script(&js);
                } else if msg == "OPEN_APP_DIR" {
                    #[cfg(windows)]
                    {
                        let _ = std::process::Command::new("explorer.exe").arg(&app_dir).spawn();
                    }
                    #[cfg(target_os = "macos")]
                    {
                        let _ = std::process::Command::new("open").arg(&app_dir).spawn();
                    }
                    #[cfg(all(unix, not(target_os = "macos")))]
                    {
                        let _ = std::process::Command::new("xdg-open").arg(&app_dir).spawn();
                    }
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
                        if !session_enabled || settings.restore_session {
                            save_session(sp, &mut last_saved, &tabs, current_tab, pinned);
                        } else {
                            let _ = fs::remove_file(sp);
                        }
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
                    if let Some((url, old_idx, was_pinned)) = closed_tabs.pop() {
                        // после закрытия последней вкладки осталась пустая стартовая - возвращённая вкладка займёт её место
                        if blank_tab && tabs.len() == 1 && is_start_page(&tabs[0], &settings.start_url) {
                            tabs.clear();
                            pinned = 0;
                        }
                        blank_tab = false;
                        if tabs.len() < MAX_TABS {
                            // закреплённая возвращается в блок закреплённых, обычная - не раньше него
                            let pos = if was_pinned { old_idx.min(pinned) } else { old_idx.max(pinned).min(tabs.len()) };
                            tabs.insert(pos, url.clone());
                            if was_pinned {
                                pinned += 1;
                            }
                            current_tab = pos;
                            cur_doc = None;
                            let label = titles.get(&url).cloned().unwrap_or_else(|| tab_label(&url));
                            window.set_title(&format!("{} — Falco", label));
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
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
                    let tabs_str = tabs_labels(&tabs, &titles, pinned);
                    let is_max = window.is_maximized() || window.fullscreen().is_some();
                    let js = format!(
                        "if(window.__ulRT) window.__ulRT('{}', {}); if(window.__ulMaxState) window.__ulMaxState({}); if(window.__ulSettings) window.__ulSettings('{}'); {}",
                        js_str(&tabs_str),
                        current_tab,
                        if is_max { "true" } else { "false" },
                        js_str(&settings.to_json()),
                        bm_state_js(&bm_map, tabs[current_tab].as_str())
                    );
                    let _ = webview.evaluate_script(&js);
                } else if msg == "new_tab" && tabs.len() < MAX_TABS {
                    let su = settings.start_url.clone();
                    tabs.push(su.clone());
                    cur_doc = None;
                    current_tab = tabs.len() - 1;
                    let tabs_str = tabs_labels(&tabs, &titles, pinned);
                    let js = format!(
                        "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                        js_str(&tabs_str),
                        current_tab,
                        nav_script(&su, true)
                    );
                    let _ = webview.evaluate_script(&js);
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB|") {
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        current_tab = tabs.len() - 1;
                        cur_doc = None;
                        let tabs_str = tabs_labels(&tabs, &titles, pinned);
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
                        let tabs_str = tabs_labels(&tabs, &titles, pinned);
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
                        let tabs_str = tabs_labels(&tabs, &titles, pinned);
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
                    open_downloads_folder(&settings.download_dir);
                } else if let Some(fp) = msg.strip_prefix("DL_SHOW|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        show_in_folder(fp, &settings.download_dir);
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
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
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
                        if from < tabs.len() && to < tabs.len() && from != to && ((from < pinned) == (to < pinned)) {
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
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg.starts_with("SWITCH_TAB|") {
                    if let Ok(idx) = msg.replace("SWITCH_TAB|", "").parse::<usize>() {
                        if idx < tabs.len() && idx != current_tab {
                            current_tab = idx;
                            cur_doc = None;
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
                        let was_pinned = idx_to_close < pinned;
                        if was_pinned {
                            pinned -= 1;
                        }
                        if closed_tabs.len() >= MAX_CLOSED_TABS {
                            closed_tabs.remove(0);
                        }
                        closed_tabs.push((closed_url, idx_to_close, was_pinned));

                        if current_tab > idx_to_close { current_tab -= 1; }
                        else if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }

                        if was_active {
                            cur_doc = None;
                            let target_url = &tabs[current_tab];
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
                            let js = format!(
                                "if(window.__ulRT) window.__ulRT('{}', {}); {}",
                                js_str(&tabs_str),
                                current_tab,
                                nav_script(target_url, false)
                            );
                            let _ = webview.evaluate_script(&js);
                        } else {
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    } else if tabs.len() == 1 && idx_to_close == 0 {
                        // Закрыли последнюю вкладку: браузер не выключаем, а открываем стартовую страницу.
                        // Закрытую вкладку можно вернуть через Ctrl+Shift+T.
                        if !is_start_page(&tabs[0], &settings.start_url) {
                            if closed_tabs.len() >= MAX_CLOSED_TABS {
                                closed_tabs.remove(0);
                            }
                            closed_tabs.push((tabs[0].clone(), 0, pinned > 0));
                        }
                        blank_tab = true;
                        tabs[0] = settings.start_url.clone();
                        pinned = 0;
                        current_tab = 0;
                        cur_doc = None;
                        hist.flush(&history_path);
                        let _ = webview.evaluate_script(&nav_script(&settings.start_url, false));
                    }
                } else if let Some(rest) = msg.strip_prefix("TAB_CMD|") {
                    // Меню правой кнопки по вкладке: "TAB_CMD|<команда>|<индекс>"
                    let (cmd, idx) = match rest.split_once('|') {
                        Some((c, i)) => (c, i.parse::<usize>().ok()),
                        None => (rest, None),
                    };
                    if let Some(idx) = idx.filter(|&i| i < tabs.len()) {
                        let mut nav = false;
                        let changed = match cmd {
                            "pin" => pin_tab(&mut tabs, &mut current_tab, &mut pinned, idx),
                            "unpin" => unpin_tab(&mut tabs, &mut current_tab, &mut pinned, idx),
                            "dup" | "new_right" if tabs.len() < MAX_TABS => {
                                let url = if cmd == "dup" { tabs[idx].clone() } else { settings.start_url.clone() };
                                insert_right(&mut tabs, &mut current_tab, pinned, idx, url);
                                nav = true;
                                true
                            }
                            "close_others" | "close_right" => {
                                let (removed, n) = if cmd == "close_others" {
                                    close_others(&mut tabs, &mut current_tab, pinned, idx)
                                } else {
                                    close_right(&mut tabs, &mut current_tab, pinned, idx)
                                };
                                nav = n;
                                let any = !removed.is_empty();
                                for (u, i) in removed {
                                    if closed_tabs.len() >= MAX_CLOSED_TABS {
                                        closed_tabs.remove(0);
                                    }
                                    closed_tabs.push((u, i, false));
                                }
                                any
                            }
                            "copy_url" => {
                                let _ = webview.evaluate_script(&format!(
                                    "if(window.__ulCopyText) window.__ulCopyText('{}');",
                                    js_str(&tabs[idx])
                                ));
                                false
                            }
                            _ => false,
                        };
                        if changed {
                            let mut js = format!(
                                "if(window.__ulRT) window.__ulRT('{}', {});",
                                js_str(&tabs_labels(&tabs, &titles, pinned)),
                                current_tab
                            );
                            if nav {
                                cur_doc = None;
                                let target_url = &tabs[current_tab];
                                let label = titles.get(target_url).cloned().unwrap_or_else(|| tab_label(target_url));
                                window.set_title(&format!("{} — Falco", label));
                                js.push(' ');
                                js.push_str(&nav_script(target_url, true));
                            }
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg == "DETACH_TAB" || msg.starts_with("DETACH_TAB|") {
                    // без индекса - текущая вкладка, с индексом - выбранная в меню
                    let di = msg
                        .strip_prefix("DETACH_TAB|")
                        .and_then(|x| x.parse::<usize>().ok())
                        .unwrap_or(current_tab);
                    let url = tabs.get(di).cloned().unwrap_or_default();
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
                                tabs.remove(di);
                                if di < pinned {
                                    pinned -= 1;
                                }
                                if di == current_tab {
                                    cur_doc = None;
                                    if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                                    let target_url = &tabs[current_tab];
                                    let _ = webview.evaluate_script(&nav_script(target_url, false));
                                } else {
                                    if di < current_tab { current_tab -= 1; }
                                    let js = format!(
                                        "if(window.__ulRT) window.__ulRT('{}', {});",
                                        js_str(&tabs_labels(&tabs, &titles, pinned)),
                                        current_tab
                                    );
                                    let _ = webview.evaluate_script(&js);
                                }
                            } else {
                                tabs[0] = settings.start_url.clone();
                                pinned = 0;
                                cur_doc = None;
                                current_tab = 0;
                                let _ = webview.evaluate_script(&nav_script(&settings.start_url, false));
                            }
                        }
                    }
                } else if let Some(id) = msg.strip_prefix("POPOUT_CLOSED|") {
                    if id.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
                        popout_alive = popout_alive.saturating_sub(1);
                        let path = app_dir.join(format!("popout-{}.txt", id));
                        let (back, _, _) = load_session(&path);
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
                            let tabs_str = tabs_labels(&tabs, &titles, pinned);
                            let js = format!("if(window.__ulRT) window.__ulRT('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    }
                } else if msg == "next_tab" {
                    if tabs.len() > 1 {
                        current_tab = (current_tab + 1) % tabs.len();
                        cur_doc = None;
                        let target_url = &tabs[current_tab];
                        let label = titles.get(target_url).cloned().unwrap_or_else(|| tab_label(target_url));
                        window.set_title(&format!("{} — Falco", label));
                        let _ = webview.evaluate_script(&nav_script(target_url, true));
                    }
                }
                if msg != "GET_TABS" && !msg.starts_with("WARM_TAB|") && msg != "SETTINGS_GET" && msg != "PICK_DL_DIR" {
                    if let Some(sp) = &save_path {
                        if !session_enabled || settings.restore_session {
                            save_session(sp, &mut last_saved, &tabs, current_tab, pinned);
                        }
                    }
                }
            }
            _ => {}
        }
    });
}
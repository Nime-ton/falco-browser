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
mod live;
#[cfg(windows)]
mod popups;
#[cfg(all(windows, feature = "native-adblock"))]
mod native_adblock;

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

/// Изменение размера безрамочного окна за край/угол. В tao 0.23 нет `drag_resize_window`,
/// поэтому на Windows запускаем штатный цикл изменения размера через WM_NCLBUTTONDOWN
/// (так же, как внутри tao работает `drag_window`). Направления: n, s, e, w, ne, nw, se, sw.
#[cfg(windows)]
fn win_resize_start(window: &tao::window::Window, dir: &str) {
    use tao::platform::windows::WindowExtWindows;
    #[repr(C)]
    struct Pt {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn ReleaseCapture() -> i32;
        fn GetCursorPos(p: *mut Pt) -> i32;
        fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    }
    // коды HT*: HTLEFT=10, HTRIGHT=11, HTTOP=12, HTTOPLEFT=13, HTTOPRIGHT=14, HTBOTTOM=15, HTBOTTOMLEFT=16, HTBOTTOMRIGHT=17
    let ht: usize = match dir {
        "w" => 10,
        "e" => 11,
        "n" => 12,
        "nw" => 13,
        "ne" => 14,
        "s" => 15,
        "sw" => 16,
        "se" => 17,
        _ => return,
    };
    unsafe {
        let mut p = Pt { x: 0, y: 0 };
        GetCursorPos(&mut p);
        ReleaseCapture();
        let lparam = (((p.y as u16 as u32) << 16) | (p.x as u16 as u32)) as isize;
        // 0x00A1 = WM_NCLBUTTONDOWN
        PostMessageW(window.hwnd() as isize, 0x00A1, ht, lparam);
    }
}

#[cfg(not(windows))]
fn win_resize_start(_window: &tao::window::Window, _dir: &str) {}

fn audio_state_js(
    tabs: &[String],
    metas: &[TabMeta],
    current_tab: usize,
    active_live: Option<u32>,
    main_playing: bool,
    lives: &[live::Live],
) -> String {
    let mut playing_idxs = Vec::new();
    let mut muted_idxs = Vec::new();
    for i in 0..tabs.len() {
        let tid = metas.get(i).map(|m| m.id).unwrap_or(0);
        let is_playing = if let Some(l) = lives.iter().find(|l| l.tab_id == tid) {
            l.playing
        } else if active_live.is_none() && i == current_tab {
            main_playing
        } else {
            false
        };
        if is_playing {
            playing_idxs.push(i.to_string());
        }
        if metas.get(i).map(|m| m.muted).unwrap_or(false) {
            muted_idxs.push(i.to_string());
        }
    }
    format!(
        "if(window.__ulAudio) window.__ulAudio([{}],[{}]);",
        playing_idxs.join(","),
        muted_idxs.join(",")
    )
}

fn shield_state_js(wl: &HashSet<String>, url: &str) -> String {
    let whitelisted = is_whitelisted_url(url, wl);
    let csv = whitelist_csv(wl);
    format!(
        "if(window.__ulShield) window.__ulShield({},'{}');",
        if whitelisted { "true" } else { "false" },
        js_str(&csv)
    )
}

/// Временная папка режима «Инкогнито»: профиль WebView2, история и список загрузок этого окна.
/// Папку охраняет открытый без общего доступа файл `.lock`: пока окно живо, другие процессы Falco её
/// не тронут; после закрытия (или падения) процесса Windows сама снимает блокировку.
fn make_incognito_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("Falco-incognito-{}-{}", std::process::id(), nanos));
    let _ = fs::create_dir_all(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        if let Ok(f) = fs::OpenOptions::new().write(true).create(true).share_mode(0).open(dir.join(".lock")) {
            std::mem::forget(f); // дескриптор остаётся открытым до конца процесса
        }
    }
    dir
}

/// Удаляет временную папку инкогнито при закрытии окна. Процессы WebView2 завершаются не мгновенно
/// и ещё держат файлы профиля, поэтому папку удаляет отдельный скрытый процесс cmd после короткой паузы.
fn purge_incognito_dir(dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let d = dir.to_string_lossy().replace('"', "");
        let script = format!(
            "ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{d}\" & ping -n 6 127.0.0.1 >nul & rmdir /s /q \"{d}\"",
            d = d
        );
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        let _ = std::process::Command::new("cmd")
            .arg("/C")
            .raw_arg(script)
            .creation_flags(0x0800_0200)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = fs::remove_dir_all(dir);
    }
}

/// Подчищает папки инкогнито, оставшиеся после аварийного завершения. Папка живого окна занята своим
/// `.lock`, её открыть не получится - такие пропускаем.
fn sweep_incognito_dirs() {
    #[cfg(windows)]
    {
        let rd = match fs::read_dir(std::env::temp_dir()) {
            Ok(r) => r,
            Err(_) => return,
        };
        for e in rd.filter_map(|e| e.ok()) {
            if !e.file_name().to_string_lossy().starts_with("Falco-incognito-") {
                continue;
            }
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            match fs::OpenOptions::new().write(true).open(dir.join(".lock")) {
                Ok(f) => {
                    drop(f);
                    let _ = fs::remove_dir_all(&dir);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // папку только что создали (lock ещё не успел появиться) - трогаем лишь старые
                    let old = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .map_or(false, |d| d.as_secs() > 120);
                    if old {
                        let _ = fs::remove_dir_all(&dir);
                    }
                }
                Err(_) => {}
            }
        }
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

    let _ = fs::create_dir_all(&app_dir);
    std::thread::spawn(sweep_incognito_dirs);

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
    // "спрашивать, куда сохранить файл": общий флаг, чтобы настройка действовала сразу, без перезапуска
    let ask_dl_shared = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(settings.ask_download));
    let dl_mgr_shared = std::sync::Arc::new(std::sync::Mutex::new(DownloadManager::default()));
    #[cfg(windows)]
    let media_mode_shared = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(settings.media_mode));

    let cli_url = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--app=").map(|s| s.trim_matches('"').to_string()))
        .or_else(|| std::env::args().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://")))
        .filter(|u| is_http_url(u));
    // --incognito: отдельный процесс без истории, сессии и общего профиля (запускается по Ctrl+Shift+N)
    let incognito = std::env::args().skip(1).any(|a| a == "--incognito");
    let session_enabled = cli_url.is_none() && !incognito;
    let app_title: &'static str = if incognito { "Falco · Инкогнито" } else { "Falco" };

    let win_state_path = app_dir.join("window.txt");
    let mut win_state = if session_enabled {
        load_win_state(&win_state_path).unwrap_or(WinState { x: 100, y: 100, w: 1280, h: 800, max: false })
    } else {
        WinState { x: 120, y: 120, w: 1100, h: 750, max: false }
    };

    let mut wb = WindowBuilder::new()
        .with_title(app_title)
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
    // окно-владелец для диалога "Сохранить как"
    #[cfg(windows)]
    let main_hwnd: isize = {
        use tao::platform::windows::WindowExtWindows;
        window.hwnd() as isize
    };
    #[cfg(not(windows))]
    let main_hwnd: isize = 0;

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
    // EasyList и EasyPrivacy вшиты в exe: раскладываем их при первом запуске (и у тех, у кого
    // уже есть только default-block.txt от старой версии).
    seed_embedded_lists(&filters_dir);

    let example = scripts_dir.join("_example.user.js");
    if !example.exists() {
        let _ = fs::write(&example, EXAMPLE_SCRIPT);
    }
    let trusted_media_path = app_dir.join("trusted-media.txt");
    if !trusted_media_path.exists() {
        let _ = fs::write(&trusted_media_path, TRUSTED_MEDIA_EXAMPLE);
    }
    let whitelist_path = app_dir.join("whitelist.txt");
    if !whitelist_path.exists() {
        let _ = fs::write(&whitelist_path, WHITELIST_EXAMPLE);
    }
    let mut whitelist = load_whitelist(&whitelist_path);
    #[cfg(all(windows, feature = "native-adblock"))]
    let whitelist_shared = std::sync::Arc::new(std::sync::RwLock::new(whitelist.clone()));

    let ipc_secret = make_secret();
    let ipc_prefix = format!("{}:", ipc_secret);
    let ipc_prefix_live = ipc_prefix.clone();
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
        None if settings.restore_session && !incognito => {
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
    // Если при запуске активна закреплённая вкладка и включены "живые" закреплённые вкладки, она получит свой WebView
    // (см. live.rs), а общий WebView пока остаётся пустым и ждёт переключения на обычную вкладку.
    let boot_live = settings.live_pinned && current_init < pinned_init;
    let start_url = if boot_live { "about:blank".to_string() } else { tabs_init[current_init].clone() };

    let ret_id: Option<String> = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--return=").map(|s| s.to_string()))
        .filter(|s| !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_digit() || b == b'_'));
    let save_path: Option<PathBuf> = if incognito {
        None
    } else if session_enabled {
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
        .replace("__WHITELIST__", &whitelist_csv(&whitelist))
        .replace("__SEARCH_URL__", settings.search_url_prefix())
        .replace("__YT_H264__", if settings.yt_force_h264 { "true" } else { "false" })
        .replace("__PIN_BAR__", if settings.pin_top_bar { "true" } else { "false" })
        .replace("__INCOGNITO__", if incognito { "true" } else { "false" });

    let cosmetic_script = format!(
        "if (window === window.top) {{ window.__ulCosmRaw = function () {{ return '{}'; }}; }}",
        js_str(&cosmetic_js)
    );

    // Инкогнито: профиль WebView2, история и список загрузок лежат во временной папке (удаляется при закрытии окна)
    let incognito_dir: Option<PathBuf> = if incognito { Some(make_incognito_dir()) } else { None };
    let profile_path = match &incognito_dir {
        Some(d) => d.join("profile"),
        None => app_dir.join("User Data"),
    };
    let mut web_context = WebContext::new(Some(profile_path));

    let proxy_waiter = proxy.clone();
    let proxy_newwin = proxy.clone();
    let last_dl_target = std::sync::Arc::new(std::sync::Mutex::new(PathBuf::new()));

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
        .with_navigation_handler(|url: String| nav_allowed(&url));

    // На не-Windows используем стандартный обработчик wry (на Windows ставится нативный ICoreWebView2_4 с прогрессом и паузой)
    #[cfg(not(windows))]
    let builder = {
        let proxy_dl_start = proxy_waiter.clone();
        let proxy_dl_done = proxy_waiter.clone();
        let last_dl_for_start = last_dl_target.clone();
        let last_dl_for_done = last_dl_target.clone();
        let dl_dir_for_handler = dl_dir_shared.clone();
        let ask_dl_for_handler = ask_dl_shared.clone();
        builder
            .with_download_started_handler(move |url: String, path: &mut PathBuf| {
                if url.starts_with("file:") {
                    return false;
                }
                let custom = dl_dir_for_handler.lock().ok().map(|g| g.clone()).unwrap_or_default();
                let ask = ask_dl_for_handler.load(std::sync::atomic::Ordering::Relaxed);
                let dest = match choose_download_path(path, &custom, ask, main_hwnd) {
                    Some(d) => d,
                    None => return false,
                };
                let name = dest
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("файл")
                    .to_string();
                let dest_str = dest.to_string_lossy().into_owned();
                if let Ok(mut guard) = last_dl_for_start.lock() {
                    *guard = dest.clone();
                }
                *path = dest;
                let _ = proxy_dl_start.send_event(format!("DL_START|0|{}|{}|{}|0", name, dest_str, url));
                true
            })
            .with_download_completed_handler(move |dl_url, path, success| {
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
                let state = if success { "ok" } else { "err" };
                let _ = proxy_dl_done.send_event(format!("DL_DONE|0|{}|{}|{}|{}|0", state, name, full_path, dl_url));
            })
    };

    let builder = builder
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
    let browser_args: String = {
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
        format!(
            "--disable-features={} --enable-features=CanvasOopRasterization,VaapiVideoDecoder,BackForwardCache,SmoothScrolling,ViewTransitionOnNavigation --enable-smooth-scrolling --disable-pinch --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --autoplay-policy=no-user-gesture-required{} --host-resolver-rules=\"{}\"",
            disabled, webrtc, rules
        )
    };
    #[cfg(not(windows))]
    let browser_args: String = String::new();
    #[cfg(windows)]
    let builder = builder.with_additional_browser_args(browser_args.clone());

    let webview = builder
        .build()
        .unwrap_or_else(|e| fatal(&app_dir, "Не удалось запустить WebView2", &format!("{:?}", e)));
    // Сбрасываем возможный старый системный зум WebView2 в 100%, чтобы панель вкладок была стандартного размера
    let _ = webview.zoom(1.0);

    #[cfg(windows)]
    let trusted_list = load_trusted_media(&trusted_media_path);
    #[cfg(windows)]
    {
        if let Err(e) = media::permissions::install(&webview, trusted_list.clone(), media_mode_shared.clone()) {
            let _ = fs::write(app_dir.join("media-permissions.log"), format!("{:?}", e));
        }
        let _ = media::audio::install(&webview, proxy_waiter.clone(), None);
        let _ = install_webview_downloads(
            &webview,
            proxy_waiter.clone(),
            dl_dir_shared.clone(),
            ask_dl_shared.clone(),
            main_hwnd,
            dl_mgr_shared.clone(),
        );
    }
    #[cfg(windows)]
    let popups_state = popups::new_state();
    #[cfg(windows)]
    {
        if let Err(e) = popups::install(&webview, popups_state.clone(), proxy_newwin.clone()) {
            let _ = fs::write(app_dir.join("popups.log"), format!("{:?}", e));
        }
    }

    #[cfg(all(windows, feature = "native-adblock"))]
    let page_url = std::rc::Rc::new(std::cell::RefCell::new(start_url.clone()));
    #[cfg(all(windows, feature = "native-adblock"))]
    let adblock_dbg_path = app_dir.join("native-adblock-debug.log");
    #[cfg(all(windows, feature = "native-adblock"))]
    let adblock_checker = {
        // движок собирается в фоновом потоке: окно появляется сразу, фильтрация включается, когда он готов
        let dbg = adblock_dbg_path.clone();
        let checker = native_adblock::spawn_engine(&filters_dir, dbg.clone());
        match native_adblock::install(&webview, checker.clone(), page_url.clone(), whitelist_shared.clone(), dbg.clone()) {
            Ok(()) => {
                if native_adblock::DEBUG {
                    native_adblock::log(&dbg, "handler installed OK\n");
                }
            }
            Err(e) => {
                let _ = fs::write(app_dir.join("native-adblock.log"), format!("{:?}", e));
            }
        }
        checker
    };

    // Фабрика WebView для закреплённых вкладок (live.rs): те же скрипты, профиль, нативный блокировщик и аргументы, что у главного WebView.
    let mut live_scripts: Vec<String> = Vec::with_capacity(2 + user_scripts.len());
    live_scripts.push(cosmetic_script);
    live_scripts.push(script);
    live_scripts.extend(user_scripts);
    let mut factory = live::Factory {
        proxy: proxy_waiter.clone(),
        ipc_prefix: ipc_prefix_live,
        scripts: live_scripts,
        browser_args,
        dl_dir: dl_dir_shared.clone(),
        ask_dl: ask_dl_shared.clone(),
        hwnd: main_hwnd,
        last_dl: last_dl_target,
        dl_mgr: dl_mgr_shared.clone(),
        ctx: web_context,
        next_id: 0,
        #[cfg(windows)]
        trusted: trusted_list,
        #[cfg(windows)]
        media_mode: media_mode_shared.clone(),
        #[cfg(windows)]
        popups: popups_state.clone(),
        #[cfg(all(windows, feature = "native-adblock"))]
        adblock: adblock_checker.clone(),
        #[cfg(all(windows, feature = "native-adblock"))]
        whitelist: whitelist_shared.clone(),
        #[cfg(all(windows, feature = "native-adblock"))]
        adblock_log: adblock_dbg_path,
    };

    let mut tabs: Vec<String> = tabs_init;
    let mut metas: Vec<TabMeta> = tabs.iter().map(|u| TabMeta::new(u)).collect();
    let mut main_playing = false;
    let mut filters_updating = false;
    // JS последнего обновления фильтров: скрипты инициализации WebView зашиты при запуске, поэтому
    // новые страницы получают свежие списки отсюда (при загрузке страницы приходит GET_TABS)
    let mut filters_hot_js = String::new();
    let mut current_tab = current_init;
    let mut pinned: usize = pinned_init; // сколько вкладок закреплено (они всегда первые)
    let mut closed_tabs: Vec<ClosedTab> = Vec::new();
    let mut blank_tab = false; // true: единственная вкладка - пустая стартовая, оставшаяся после закрытия последней
    let mut last_saved = String::new();
    let mut titles: HashMap<String, String> = HashMap::new();
    let data_dir = incognito_dir.clone().unwrap_or_else(|| app_dir.clone());
    let history_path = data_dir.join("history.txt");
    let bookmarks_path = app_dir.join("bookmarks.txt");
    let downloads_path = data_dir.join("downloads.txt");
    // иконки сайтов для вкладок: переживают переходы между вкладками (каждая обычная вкладка - новый документ) и перезапуск
    let favs_path = data_dir.join("favicons.txt");
    let mut favs: HashMap<String, String> = load_favs(&favs_path);
    if !incognito {
        compact_history(&history_path);
    }
    let mut hist = HistState::new(settings.history_enabled && !incognito);
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
    let mut lives: Vec<live::Live> = Vec::new(); // WebView закреплённых вкладок
    let mut active_live: Option<u32> = None; // id показанного закреплённого WebView (None - показан общий)
    let mut live_failed: Option<u64> = None; // tab_id вкладки, для которой WebView создать не удалось
    if boot_live {
        // создаст WebView активной закреплённой вкладки, как только запустится главный цикл
        let _ = proxy_waiter.send_event("LIVE_BOOT".to_string());
    }

    event_loop.run(move |event, elwt, control_flow| {
        *control_flow = ControlFlow::Wait;
        #[cfg(not(windows))]
        let _ = elwt;

        // evaluate_script на том WebView, который сейчас показан (общий или закреплённой вкладки)
        macro_rules! ev {
            ($js:expr) => {{
                let js_ref = $js;
                match active_live.and_then(|id| lives.iter().find(|l| l.id == id)) {
                    Some(l) => l.view.evaluate_script(js_ref),
                    None => webview.evaluate_script(js_ref),
                }
            }};
        }
        // evaluate_script на всех живых WebView (чтобы активные загрузки были синхронизированы при переключении вкладок)
        macro_rules! ev_all {
            ($js:expr) => {{
                let js_ref = $js;
                let _ = webview.evaluate_script(js_ref);
                for l in &lives {
                    let _ = l.view.evaluate_script(js_ref);
                }
            }};
        }
        // синхронизировать состояние Mute на WebView вкладок
        macro_rules! sync_mute {
            () => {{
                ensure_metas(&tabs, &mut metas);
                if active_live.is_none() {
                    let m = metas.get(current_tab).map(|m| m.muted).unwrap_or(false);
                    media::audio::set_muted(&webview, m);
                }
                for l in &lives {
                    if let Some(m) = metas.iter().find(|m| m.id == l.tab_id) {
                        media::audio::set_muted(&l.view, m.muted);
                    }
                }
            }};
        }
        // показать WebView для вкладки current_tab; true - показан закреплённый (страница уже живая)
        macro_rules! sel {
            () => {{
                ensure_metas(&tabs, &mut metas);
                let shown = live::select(
                    &window,
                    &webview,
                    &mut factory,
                    &mut lives,
                    &mut active_live,
                    &mut cur_doc,
                    &tabs,
                    &metas,
                    pinned,
                    current_tab,
                    settings.live_pinned,
                );
                if !shown {
                    main_playing = false;
                }
                sync_mute!();
                shown
            }};
        }
        macro_rules! tabs_rt {
            () => {{
                ensure_metas(&tabs, &mut metas);
                format!(
                    "if(window.__ulRT) window.__ulRT('{}', {}, '{}'); {} {}",
                    js_str(&tabs_labels(&tabs, &titles, pinned)),
                    current_tab,
                    js_str(&tabs_hosts(&tabs)),
                    audio_state_js(&tabs, &metas, current_tab, active_live, main_playing, &lives),
                    nav_state_js(&metas, current_tab)
                )
            }};
        }
        // то же, что браузерная панель получает при GET_TABS: нужно уже загруженной закреплённой странице
        macro_rules! live_extra {
            () => {{
                let is_max_now = window.is_maximized() || window.fullscreen().is_some();
                format!(
                    "{} if(window.__ulMaxState) window.__ulMaxState({}); if(window.__ulSettings) window.__ulSettings('{}'); {} {}",
                    tabs_rt!(),
                    if is_max_now { "true" } else { "false" },
                    js_str(&settings.to_json()),
                    bm_state_js(&bm_map, tabs[current_tab].as_str()),
                    shield_state_js(&whitelist, tabs[current_tab].as_str())
                )
            }};
        }
        // замена nav_script: для обычной вкладки - переход общего WebView (как раньше), для закреплённой - переключение на её WebView
        macro_rules! nav_target {
            ($url:expr, $save:expr) => {{
                ensure_metas(&tabs, &mut metas);
                if let Some(m) = metas.get_mut(current_tab) {
                    m.hist.in_step = true;
                }
                if sel!() {
                    live_extra!()
                } else {
                    nav_script($url, $save)
                }
            }};
        }

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
                live::on_resize(&window, &lives, active_live);
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
                if let Some(d) = &incognito_dir {
                    purge_incognito_dir(d);
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(msg) => {
                ensure_metas(&tabs, &mut metas);
                // Сообщения закреплённых WebView приходят с меткой "LV<id>~". Показанного WebView они обрабатываются как
                // обычные, от фоновых берём только адрес страницы и статус звука. Страничные сообщения скрытого общего WebView
                // игнорируем, пока показана закреплённая вкладка.
                let mut msg = msg;
                match live::split_tag(&msg).map(|(i, s)| (i, s.to_string())) {
                    Some((id, inner)) => {
                        if let Some(p_str) = inner.strip_prefix("AUDIO|") {
                            if let Some(l) = lives.iter_mut().find(|l| l.id == id) {
                                l.playing = p_str == "1";
                            }
                            let _ = ev!(&audio_state_js(&tabs, &metas, current_tab, active_live, main_playing, &lives));
                            return;
                        }
                        if active_live == Some(id) {
                            msg = inner;
                        } else {
                            live::background_msg(&mut lives, &mut tabs, &mut metas, pinned, id, &inner);
                            return;
                        }
                    }
                    None => {
                        if let Some(p_str) = msg.strip_prefix("AUDIO|") {
                            main_playing = p_str == "1";
                            let _ = ev!(&audio_state_js(&tabs, &metas, current_tab, active_live, main_playing, &lives));
                            return;
                        }
                        if active_live.is_some() && live::is_page_doc_msg(&msg) {
                            return;
                        }
                    }
                }
                let msg = match filter_doc_msg(&msg, &mut cur_doc) {
                    Some(m) => m,
                    None => return,
                };
                if !msg.starts_with("SYNC_URL|")
                    && !msg.starts_with("POPOUT_CLOSED|")
                    && !msg.starts_with("DL_START|")
                    && !msg.starts_with("DL_PROG|")
                    && !msg.starts_with("DL_DONE|")
                    && !msg.starts_with("FILTERS_UPDATED|")
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
                    window.set_title(&format!("{} — {}", label, app_title));
                    if let Some(id) = active_live {
                        if let Some(l) = lives.iter_mut().find(|l| l.id == id) {
                            l.url = url.clone();
                            #[cfg(all(windows, feature = "native-adblock"))]
                            {
                                *l.page_url.borrow_mut() = url.clone();
                            }
                        }
                    }
                    tabs[current_tab] = url.clone();
                    if let Some(m) = metas.get_mut(current_tab) {
                        m.hist.record(&url);
                    }
                    let _ = ev!(&format!(
                        "{} {} {}",
                        tabs_rt!(),
                        bm_state_js(&bm_map, &url),
                        shield_state_js(&whitelist, &url)
                    ));
                } else if msg == "NAV_BACK" {
                    if let Some(target_url) = metas.get_mut(current_tab).and_then(|m| m.hist.step_back()) {
                        cur_doc = None;
                        tabs[current_tab] = target_url.clone();
                        #[cfg(all(windows, feature = "native-adblock"))]
                        {
                            *page_url.borrow_mut() = target_url.clone();
                        }
                        if let Some(id) = active_live {
                            if let Some(l) = lives.iter_mut().find(|l| l.id == id) {
                                l.url = target_url.clone();
                                #[cfg(all(windows, feature = "native-adblock"))]
                                {
                                    *l.page_url.borrow_mut() = target_url.clone();
                                }
                            }
                        }
                        let label = titles.get(&target_url).cloned().unwrap_or_else(|| tab_label(&target_url));
                        window.set_title(&format!("{} — {}", label, app_title));
                        let js = format!("{} {}", tabs_rt!(), nav_script(&target_url, true));
                        let _ = ev!(&js);
                    }
                } else if msg == "NAV_FWD" {
                    if let Some(target_url) = metas.get_mut(current_tab).and_then(|m| m.hist.step_forward()) {
                        cur_doc = None;
                        tabs[current_tab] = target_url.clone();
                        #[cfg(all(windows, feature = "native-adblock"))]
                        {
                            *page_url.borrow_mut() = target_url.clone();
                        }
                        if let Some(id) = active_live {
                            if let Some(l) = lives.iter_mut().find(|l| l.id == id) {
                                l.url = target_url.clone();
                                #[cfg(all(windows, feature = "native-adblock"))]
                                {
                                    *l.page_url.borrow_mut() = target_url.clone();
                                }
                            }
                        }
                        let label = titles.get(&target_url).cloned().unwrap_or_else(|| tab_label(&target_url));
                        window.set_title(&format!("{} — {}", label, app_title));
                        let js = format!("{} {}", tabs_rt!(), nav_script(&target_url, true));
                        let _ = ev!(&js);
                    }
                } else if msg == "SHIELD_TOGGLE" {
                    let cur_url = tabs.get(current_tab).cloned().unwrap_or_default();
                    if let Some(now_whitelisted) = toggle_whitelist(&whitelist_path, &mut whitelist, &cur_url) {
                        #[cfg(all(windows, feature = "native-adblock"))]
                        if let Ok(mut w) = whitelist_shared.write() {
                            *w = whitelist.clone();
                        }
                        let sh_js = shield_state_js(&whitelist, &cur_url);
                        ev_all!(&sh_js);
                        let host = host_of(&cur_url).unwrap_or_default();
                        let notice = if now_whitelisted {
                            format!("Защита отключена для {}", host)
                        } else {
                            format!("Защита включена для {}", host)
                        };
                        let reload_js = format!(
                            "{} if(window.__ulDlNotice) window.__ulDlNotice('info','{}',''); setTimeout(function(){{ location.reload(); }}, 180);",
                            sh_js,
                            js_str(&notice)
                        );
                        let _ = ev!(&reload_js);
                    }
                } else if msg == "FILTERS_UPDATE" {
                    if !filters_updating {
                        filters_updating = true;
                        let start_js = "if(window.__ulFilterUpdateState) window.__ulFilterUpdateState('busy','Скачивание списков…'); if(window.__ulDlNotice) window.__ulDlNotice('start','Обновление фильтров…','');";
                        let _ = ev!(start_js);
                        let f_dir = filters_dir.clone();
                        let p = proxy_waiter.clone();
                        std::thread::spawn(move || {
                            match update_filter_lists(&f_dir) {
                                Ok(n) => {
                                    let _ = p.send_event(format!("FILTERS_UPDATED|1|{}", n));
                                }
                                Err(err) => {
                                    let _ = p.send_event(format!("FILTERS_UPDATED|0|{}", err));
                                }
                            }
                        });
                    }
                } else if let Some(rest) = msg.strip_prefix("FILTERS_UPDATED|") {
                    filters_updating = false;
                    let (ok, payload) = rest.split_once('|').unwrap_or(("0", "Ошибка обновления"));
                    if ok == "1" {
                        let new_filters = load_filters(&filters_dir);
                        let new_ad_hosts = new_filters.js.join(",");
                        let new_cosm = cosmetic_json(&new_filters);
                        if !factory.scripts.is_empty() {
                            factory.scripts[0] = format!(
                                "if (window === window.top) {{ window.__ulCosmRaw = function () {{ return '{}'; }}; }}",
                                js_str(&new_cosm)
                            );
                        }
                        #[cfg(all(windows, feature = "native-adblock"))]
                        adblock_checker.reload();

                        let upd_js = format!(
                            "if(window.__ulUpdateFilters) window.__ulUpdateFilters('{}','{}');",
                            js_str(&new_ad_hosts),
                            js_str(&new_cosm)
                        );
                        ev_all!(&upd_js);
                        filters_hot_js = upd_js;
                        let done_msg = format!("Фильтры обновлены (списков: {})", payload);
                        let notice_js = format!(
                            "if(window.__ulFilterUpdateState) window.__ulFilterUpdateState('ok','{}'); if(window.__ulDlNotice) window.__ulDlNotice('ok','{}','');",
                            js_str(&done_msg),
                            js_str(&done_msg)
                        );
                        let _ = ev!(&notice_js);
                    } else {
                        let err_msg = format!("Ошибка: {}", payload);
                        let notice_js = format!(
                            "if(window.__ulFilterUpdateState) window.__ulFilterUpdateState('err','{}'); if(window.__ulDlNotice) window.__ulDlNotice('err','Не удалось обновить фильтры','');",
                            js_str(&err_msg)
                        );
                        let _ = ev!(&notice_js);
                    }
                } else if let Some(idx_str) = msg.strip_prefix("MUTE_TAB|") {
                    let idx = idx_str.parse::<usize>().ok().unwrap_or(current_tab);
                    if idx < tabs.len() {
                        ensure_metas(&tabs, &mut metas);
                        metas[idx].muted = !metas[idx].muted;
                        sync_mute!();
                        let _ = ev!(&audio_state_js(&tabs, &metas, current_tab, active_live, main_playing, &lives));
                    }
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
                            let _ = ev!(&suggest_js(seq, &list));
                        }
                    }
                } else if msg == "SETTINGS_GET" {
                    let js = format!(
                        "if(window.__ulSettings) window.__ulSettings('{}');",
                        js_str(&settings.to_json())
                    );
                    let _ = ev!(&js);
                } else if msg == "PICK_DL_DIR" {
                    if let Some(folder) = pick_download_folder(&window) {
                        let js = format!(
                            "if(window.__ulPickedDlDir) window.__ulPickedDlDir('{}');",
                            js_str(&folder)
                        );
                        let _ = ev!(&js);
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
                                    hist.enabled = settings.history_enabled && !incognito;
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
                                "live_pinned" => settings.live_pinned = v == "1",
                                "ask_download" => {
                                    settings.ask_download = v == "1";
                                    ask_dl_shared.store(settings.ask_download, std::sync::atomic::Ordering::Relaxed);
                                }
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
                    let _ = ev!(&js);
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
                        let _ = ev!(&format!(
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
                    if let Some(d) = &incognito_dir {
                        purge_incognito_dir(d);
                    }
                    *control_flow = ControlFlow::Exit;
                } else if msg == "WIN_DRAG" {
                    let _ = window.drag_window();
                } else if let Some(dir_str) = msg.strip_prefix("WIN_RESIZE|") {
                    if !window.is_maximized() && window.fullscreen().is_none() {
                        win_resize_start(&window, dir_str);
                    }
                } else if msg == "REOPEN_TAB" {
                    if let Some(closed) = closed_tabs.pop() {
                        // после закрытия последней вкладки осталась пустая стартовая - возвращённая вкладка займёт её место
                        if blank_tab && tabs.len() == 1 && is_start_page(&tabs[0], &settings.start_url) {
                            tabs.clear();
                            metas.clear();
                            pinned = 0;
                        }
                        blank_tab = false;
                        if tabs.len() < MAX_TABS {
                            // закреплённая возвращается в блок закреплённых, обычная - не раньше него
                            let pos = if closed.was_pinned {
                                closed.index.min(pinned)
                            } else {
                                closed.index.max(pinned).min(tabs.len())
                            };
                            let url = closed.url;
                            let mut meta = TabMeta::new(&url);
                            meta.hist = closed.hist;
                            meta.hist.in_step = true;
                            tabs.insert(pos, url.clone());
                            metas.insert(pos, meta);
                            if closed.was_pinned {
                                pinned += 1;
                            }
                            current_tab = pos;
                            cur_doc = None;
                            let label = titles.get(&url).cloned().unwrap_or_else(|| tab_label(&url));
                            window.set_title(&format!("{} — {}", label, app_title));
                            let nav_js = nav_target!(&url, true);
                            let js = format!("{} {}", tabs_rt!(), nav_js);
                            let _ = ev!(&js);
                        }
                    }
                } else if let Some(idx_str) = msg.strip_prefix("WARM_TAB|") {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        if let Some(u) = tabs.get(idx) {
                            if let Some(h) = host_of(u) {
                                let scheme = if u.starts_with("http://") { "http://" } else { "https://" };
                                let origin = format!("{}{}", scheme, h);
                                let _ = ev!(&format!(
                                    "if(window.__ulWarmHost) window.__ulWarmHost('{}');",
                                    js_str(&origin)
                                ));
                            }
                        }
                    }
                } else if let Some(rest) = msg.strip_prefix("FAV|") {
                    // "FAV|<хост>|<base64 PNG>" - иконка сайта; "FAV|<хост>|!" - получить иконку не удалось
                    if let Some((k, v)) = rest.split_once('|') {
                        let known = tabs.iter().any(|u| fav_key(u).as_deref() == Some(k));
                        if known && valid_domain(k) && (v == "!" || fav_value_ok(v)) {
                            let new_val: Option<String> = if v == "!" {
                                match favs.get(k) {
                                    None => Some("!1".to_string()),
                                    Some(o) if o.starts_with('!') => {
                                        let n = o[1..].parse::<u8>().unwrap_or(0).saturating_add(1).min(3);
                                        Some(format!("!{}", n))
                                    }
                                    Some(_) => None, // настоящая иконка уже есть
                                }
                            } else if v.starts_with('!') {
                                None
                            } else if favs.get(k).map(|o| o.as_str()) == Some(v) {
                                None
                            } else {
                                Some(v.to_string())
                            };
                            if let Some(val) = new_val {
                                if !favs.contains_key(k) && favs.len() >= FAV_MAX_ENTRIES {
                                    favs.retain(|h, _| tabs.iter().any(|u| fav_key(u).as_deref() == Some(h.as_str())));
                                }
                                let persist = !val.starts_with('!');
                                favs.insert(k.to_string(), val);
                                if persist {
                                    save_favs(&favs_path, &favs);
                                }
                                let _ = ev!(&favs_js_for(&favs, &[k.to_string()]));
                            }
                        }
                    }
                } else if msg == "GET_TABS" || msg == "GET_TABS|F" {
                    // "GET_TABS|F" - первый запрос нового документа: вместе с вкладками отправляем и кэш иконок
                    let is_max = window.is_maximized() || window.fullscreen().is_some();
                    let mut js = format!(
                        "{} if(window.__ulMaxState) window.__ulMaxState({}); if(window.__ulSettings) window.__ulSettings('{}'); {} {}",
                        tabs_rt!(),
                        if is_max { "true" } else { "false" },
                        js_str(&settings.to_json()),
                        bm_state_js(&bm_map, tabs[current_tab].as_str()),
                        shield_state_js(&whitelist, tabs[current_tab].as_str())
                    );
                    if !filters_hot_js.is_empty() {
                        js.push(' ');
                        js.push_str(&filters_hot_js);
                    }
                    if msg == "GET_TABS|F" {
                        // иконки ДО вкладок: страница успевает начать их декодирование, и панель рисуется сразу с иконками
                        js = format!("{} {}", favs_js(&favs, &tabs), js);
                    }
                    let _ = ev!(&js);
                } else if msg == "new_tab" && tabs.len() < MAX_TABS {
                    let su = settings.start_url.clone();
                    tabs.push(su.clone());
                    metas.push(TabMeta::new(&su));
                    cur_doc = None;
                    current_tab = tabs.len() - 1;
                    let nav_js = nav_target!(&su, true);
                    let js = format!("{} {}", tabs_rt!(), nav_js);
                    let _ = ev!(&js);
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB|") {
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        metas.push(TabMeta::new(url));
                        current_tab = tabs.len() - 1;
                        cur_doc = None;
                        let nav_js = nav_target!(url, true);
                        let js = format!("{} {}", tabs_rt!(), nav_js);
                        let _ = ev!(&js);
                    }
                } else if msg == "INCOGNITO" {
                    // Ctrl+Shift+N: отдельный процесс со своим временным профилем
                    if spawn_ok(&mut last_spawn) {
                        let _ = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--incognito").spawn());
                    }
                } else if incognito && (msg.starts_with("OPEN_WINDOW|") || msg == "DETACH_TAB" || msg.starts_with("DETACH_TAB|")) {
                    // новый процесс без --incognito записал бы данные в обычный профиль, с ним - потерял бы вход на сайтах
                    let _ = ev!("if(window.__ulDlNotice) window.__ulDlNotice('info','В режиме инкогнито недоступно','');");
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
                        metas.push(TabMeta::new(url));
                        let js = format!("{} if(window.__ulFlash) window.__ulFlash();", tabs_rt!());
                        let _ = ev!(&js);
                    }
                } else if let Some(url) = msg.strip_prefix("OPEN_TAB_BG|") {
                    if is_http_url(url) && tabs.len() < MAX_TABS {
                        tabs.push(url.to_string());
                        metas.push(TabMeta::new(url));
                        let js = format!("{} if(window.__ulFlash) window.__ulFlash();", tabs_rt!());
                        let _ = ev!(&js);
                    }
                } else if let Some(rest) = msg.strip_prefix("DL_START|") {
                    // "DL_START|id|name|path|url|total"
                    let mut it = rest.splitn(5, '|');
                    let id = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                    let name = it.next().unwrap_or("файл");
                    let full_path = it.next().unwrap_or("");
                    let src_url = it.next().unwrap_or("");
                    let total = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let js = format!(
                        "if(window.__ulDlLive) window.__ulDlLive('start',{},{},{},{},false,'{}','{}','{}');",
                        id,
                        0,
                        total.max(0),
                        0,
                        js_str(name),
                        js_str(full_path),
                        js_str(src_url)
                    );
                    ev_all!(&js);
                } else if let Some(rest) = msg.strip_prefix("DL_PROG|") {
                    // "DL_PROG|id|rec|tot|spd|paused|name|path|url"
                    let mut it = rest.splitn(8, '|');
                    let id = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                    let rec = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let tot = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let spd = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let paused = it.next().unwrap_or("0") == "1";
                    let name = it.next().unwrap_or("файл");
                    let full_path = it.next().unwrap_or("");
                    let src_url = it.next().unwrap_or("");
                    let js = format!(
                        "if(window.__ulDlLive) window.__ulDlLive('prog',{},{},{},{},{},'{}','{}','{}');",
                        id,
                        rec.max(0),
                        tot.max(0),
                        spd.max(0),
                        if paused { "true" } else { "false" },
                        js_str(name),
                        js_str(full_path),
                        js_str(src_url)
                    );
                    ev_all!(&js);
                } else if let Some(rest) = msg.strip_prefix("DL_DONE|") {
                    // "DL_DONE|id|state(ok/cancel/err)|name|path|url|bytes"
                    let mut it = rest.splitn(6, '|');
                    let id = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                    let state = it.next().unwrap_or("err");
                    let name = it.next().unwrap_or("файл");
                    let full_path = it.next().unwrap_or("");
                    let src_url = it.next().unwrap_or("");
                    let bytes = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);

                    if let Ok(mut mgr) = dl_mgr_shared.lock() {
                        mgr.remove(id);
                    }

                    if !full_path.is_empty() && state != "cancel" {
                        let status_title = if state == "ok" {
                            format!("✔ {}", name)
                        } else {
                            format!("✖ {} (ошибка)", name)
                        };
                        dl_add(&downloads_path, full_path, &status_title, src_url);
                    }
                    let list = dl_view(&downloads_path);
                    let js = format!(
                        "if(window.__ulDlLive) window.__ulDlLive('{}',{},{},{},0,false,'{}','{}','{}'); {}",
                        state,
                        id,
                        bytes.max(0),
                        bytes.max(0),
                        js_str(name),
                        js_str(full_path),
                        js_str(src_url),
                        dl_panel_js(&list)
                    );
                    ev_all!(&js);
                } else if let Some(id_str) = msg.strip_prefix("DL_PAUSE|") {
                    if let Ok(id) = id_str.parse::<u32>() {
                        if let Ok(mgr) = dl_mgr_shared.lock() {
                            mgr.pause(id);
                        }
                    }
                } else if let Some(id_str) = msg.strip_prefix("DL_RESUME|") {
                    if let Ok(id) = id_str.parse::<u32>() {
                        if let Ok(mgr) = dl_mgr_shared.lock() {
                            mgr.resume(id);
                        }
                    }
                } else if let Some(id_str) = msg.strip_prefix("DL_CANCEL|") {
                    if let Ok(id) = id_str.parse::<u32>() {
                        if let Ok(mgr) = dl_mgr_shared.lock() {
                            mgr.cancel(id);
                        }
                    }
                } else if msg == "DL_LIST" {
                    let list = dl_view(&downloads_path);
                    let _ = ev!(&dl_panel_js(&list));
                } else if msg == "DL_FOLDER" {
                    open_downloads_folder(&settings.download_dir);
                } else if let Some(fp) = msg.strip_prefix("DL_DRAG|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN && !fp.chars().any(|c| c.is_control()) {
                        start_file_drag(fp);
                    }
                } else if let Some(fp) = msg.strip_prefix("DL_OPEN|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        open_downloaded_file(fp);
                    }
                } else if let Some(fp) = msg.strip_prefix("DL_SHOW|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        show_in_folder(fp, &settings.download_dir);
                    }
                } else if let Some(fp) = msg.strip_prefix("DL_DEL|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        dl_del(&downloads_path, fp);
                        let list = dl_view(&downloads_path);
                        ev_all!(&dl_panel_js(&list));
                    }
                } else if let Some(fp) = msg.strip_prefix("DL_DEL_FILE|") {
                    if !fp.is_empty() && fp.len() <= MAX_URL_LEN {
                        delete_downloaded_file(&downloads_path, fp);
                        let list = dl_view(&downloads_path);
                        let js = format!(
                            "{} if(window.__ulDlNotice) window.__ulDlNotice('info','Файл удалён с диска','');",
                            dl_panel_js(&list)
                        );
                        ev_all!(&js);
                    }
                } else if let Some(u) = msg.strip_prefix("DL_RETRY|") {
                    if is_http_url(u) {
                        let js = format!(
                            "try {{ const a = document.createElement('a'); a.href = '{}'; a.download = ''; document.body.appendChild(a); a.click(); a.remove(); }} catch(e) {{ window.location.href = '{}'; }}",
                            js_str(u),
                            js_str(u)
                        );
                        let _ = ev!(&js);
                    }
                } else if msg == "DL_CLEAR" {
                    let _ = fs::remove_file(&downloads_path);
                    ev_all!(&dl_panel_js(&[]));
                } else if let Some(rest) = msg.strip_prefix("PERF|") {
                    if PERF_LOG && perf_lines < 500 {
                        perf_lines += 1;
                        let clean: String = rest.chars().filter(|c| !c.is_control()).take(200).collect();
                        perf_log(&app_dir, &format!("page: init|ttfb|dcl|load|host = {}", clean));
                    }
                } else if msg == "HIST_LIST" {
                    hist.flush(&history_path);
                    let list = history_view(&history_path, HISTORY_SHOW);
                    let _ = ev!(&panel_js("history", &list));
                } else if let Some(u) = msg.strip_prefix("HIST_DEL|") {
                    if is_http_url(u) {
                        hist.flush(&history_path);
                        history_del(&history_path, u);
                        hist.last = None;
                        let list = history_view(&history_path, HISTORY_SHOW);
                        let _ = ev!(&panel_js("history", &list));
                    }
                } else if msg == "HIST_CLEAR" {
                    hist.pending = None;
                    hist.last = None;
                    let _ = fs::remove_file(&history_path);
                    let _ = ev!(&panel_js("history", &[]));
                } else if msg == "BM_LIST" {
                    let list = bm_view(&bookmarks_path);
                    bm_map = list.iter().map(|e| (e.url.clone(), e.title.clone())).collect();
                    let _ = ev!(&panel_js("bookmarks", &list));
                } else if let Some(rest) = msg.strip_prefix("BM_ADD|") {
                    if let Some((u, t)) = rest.split_once(FIELD_SEP) {
                        if is_http_url(u) {
                            let title = bm_add(&bookmarks_path, u, &clean_title(t));
                            bm_map.insert(u.to_string(), title);
                            let _ = ev!(&bm_state_js(&bm_map, u));
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
                        let _ = ev!(&js);
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
                            let _ = ev!(&tabs_rt!());
                        }
                        window.set_title(&format!("{} — {}", t, app_title));
                    }
                } else if let Some(rest) = msg.strip_prefix("MOVE_TAB|") {
                    let mut it = rest.split('|');
                    let from = it.next().and_then(|x| x.parse::<usize>().ok());
                    let to = it.next().and_then(|x| x.parse::<usize>().ok());
                    if let (Some(from), Some(to)) = (from, to) {
                        if from < tabs.len() && to < tabs.len() && from != to && ((from < pinned) == (to < pinned)) {
                            let moved = tabs.remove(from);
                            tabs.insert(to, moved);
                            let mv_m = metas.remove(from);
                            metas.insert(to, mv_m);
                            current_tab = if current_tab == from {
                                to
                            } else {
                                let mut c = current_tab;
                                if from < c { c -= 1; }
                                if to <= c { c += 1; }
                                c
                            };
                            let _ = ev!(&tabs_rt!());
                        }
                    }
                } else if msg.starts_with("SWITCH_TAB|") {
                    if let Ok(idx) = msg.replace("SWITCH_TAB|", "").parse::<usize>() {
                        if idx < tabs.len() && idx != current_tab {
                            current_tab = idx;
                            cur_doc = None;
                            let target_url = tabs[current_tab].clone();
                            let label = titles.get(&target_url).cloned().unwrap_or_else(|| tab_label(&target_url));
                            window.set_title(&format!("{} — {}", label, app_title));
                            let _ = ev!(&nav_target!(&target_url, true));
                        }
                    }
                } else if msg.starts_with("CLOSE_TAB|") {
                    let mut idx_to_close = current_tab;
                    let parsed = msg.replace("CLOSE_TAB|", "").parse::<i32>().unwrap_or(-1);
                    if parsed >= 0 { idx_to_close = parsed as usize; }

                    if tabs.len() > 1 && idx_to_close < tabs.len() {
                        let was_active = idx_to_close == current_tab;
                        let closed_url = tabs.remove(idx_to_close);
                        let closed_meta = metas.remove(idx_to_close);
                        let was_pinned = idx_to_close < pinned;
                        if was_pinned {
                            pinned -= 1;
                        }
                        if closed_tabs.len() >= MAX_CLOSED_TABS {
                            closed_tabs.remove(0);
                        }
                        closed_tabs.push(ClosedTab {
                            url: closed_url,
                            index: idx_to_close,
                            was_pinned,
                            hist: closed_meta.hist,
                        });

                        if current_tab > idx_to_close { current_tab -= 1; }
                        else if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }

                        if was_active {
                            cur_doc = None;
                            let target_url = tabs[current_tab].clone();
                            let nav_js = nav_target!(&target_url, false);
                            let js = format!("{} {}", tabs_rt!(), nav_js);
                            let _ = ev!(&js);
                        } else {
                            let _ = ev!(&tabs_rt!());
                        }
                    } else if tabs.len() == 1 && idx_to_close == 0 {
                        // Закрыли последнюю вкладку: браузер не выключаем, а открываем стартовую страницу.
                        // Закрытую вкладку можно вернуть через Ctrl+Shift+T.
                        if !is_start_page(&tabs[0], &settings.start_url) {
                            if closed_tabs.len() >= MAX_CLOSED_TABS {
                                closed_tabs.remove(0);
                            }
                            let old_hist = metas.get(0).map(|m| m.hist.clone()).unwrap_or_else(|| TabHist::new(&tabs[0]));
                            closed_tabs.push(ClosedTab {
                                url: tabs[0].clone(),
                                index: 0,
                                was_pinned: pinned > 0,
                                hist: old_hist,
                            });
                        }
                        blank_tab = true;
                        tabs[0] = settings.start_url.clone();
                        metas = vec![TabMeta::new(&settings.start_url)];
                        pinned = 0;
                        current_tab = 0;
                        cur_doc = None;
                        hist.flush(&history_path);
                        let su = settings.start_url.clone();
                        let _ = ev!(&nav_target!(&su, false));
                    }
                } else if let Some(rest) = msg.strip_prefix("TAB_CMD|") {
                    // Меню правой кнопки по вкладке: "TAB_CMD|<команда>|<индекс>"
                    let (cmd, idx) = match rest.split_once('|') {
                        Some((c, i)) => (c, i.parse::<usize>().ok()),
                        None => (rest, None),
                    };
                    if let Some(idx) = idx.filter(|&i| i < tabs.len()) {
                        let mut nav = false;
                        ensure_metas(&tabs, &mut metas);
                        let changed = match cmd {
                            "pin" => {
                                let at = pinned;
                                let ok = pin_tab(&mut tabs, &mut current_tab, &mut pinned, idx);
                                if ok && idx < metas.len() {
                                    let m = metas.remove(idx);
                                    metas.insert(at, m);
                                }
                                ok
                            }
                            "unpin" => {
                                let ok = unpin_tab(&mut tabs, &mut current_tab, &mut pinned, idx);
                                if ok && idx < metas.len() {
                                    let m = metas.remove(idx);
                                    metas.insert(pinned, m);
                                }
                                ok
                            }
                            "dup" | "new_right" if tabs.len() < MAX_TABS => {
                                let url = if cmd == "dup" { tabs[idx].clone() } else { settings.start_url.clone() };
                                let mut m = TabMeta::new(&url);
                                if cmd == "dup" {
                                    m.hist = metas[idx].hist.clone();
                                    m.hist.in_step = true;
                                }
                                let at = insert_right(&mut tabs, &mut current_tab, pinned, idx, url);
                                metas.insert(at, m);
                                nav = true;
                                true
                            }
                            "close_others" | "close_right" => {
                                let (removed, n) = if cmd == "close_others" {
                                    close_others_meta(&mut tabs, &mut metas, &mut current_tab, pinned, idx)
                                } else {
                                    close_right_meta(&mut tabs, &mut metas, &mut current_tab, pinned, idx)
                                };
                                nav = n;
                                let any = !removed.is_empty();
                                for c in removed {
                                    if closed_tabs.len() >= MAX_CLOSED_TABS {
                                        closed_tabs.remove(0);
                                    }
                                    closed_tabs.push(c);
                                }
                                any
                            }
                            "mute" => {
                                metas[idx].muted = !metas[idx].muted;
                                sync_mute!();
                                true
                            }
                            "pip" => {
                                let tid = metas[idx].id;
                                if let Some(l) = lives.iter().find(|l| l.tab_id == tid) {
                                    let _ = l.view.evaluate_script("if(window.__ulTogglePiP) window.__ulTogglePiP();");
                                } else if idx == current_tab {
                                    let _ = ev!("if(window.__ulTogglePiP) window.__ulTogglePiP();");
                                } else {
                                    let _ = ev!("if(window.__ulDlNotice) window.__ulDlNotice('info','Переключитесь на вкладку для запуска PiP','');");
                                }
                                false
                            }
                            "copy_url" => {
                                let _ = ev!(&format!(
                                    "if(window.__ulCopyText) window.__ulCopyText('{}');",
                                    js_str(&tabs[idx])
                                ));
                                false
                            }
                            _ => false,
                        };
                        if changed {
                            let nav_js = if nav {
                                cur_doc = None;
                                let target_url = tabs[current_tab].clone();
                                let label = titles.get(&target_url).cloned().unwrap_or_else(|| tab_label(&target_url));
                                window.set_title(&format!("{} — {}", label, app_title));
                                Some(nav_target!(&target_url, true))
                            } else {
                                None
                            };
                            let mut js = tabs_rt!();
                            if let Some(njs) = nav_js {
                                js.push(' ');
                                js.push_str(&njs);
                            }
                            let _ = ev!(&js);
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
                                if di < metas.len() {
                                    metas.remove(di);
                                }
                                if di < pinned {
                                    pinned -= 1;
                                }
                                if di == current_tab {
                                    cur_doc = None;
                                    if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                                    let target_url = tabs[current_tab].clone();
                                    let _ = ev!(&nav_target!(&target_url, false));
                                } else {
                                    if di < current_tab { current_tab -= 1; }
                                    let _ = ev!(&tabs_rt!());
                                }
                            } else {
                                tabs[0] = settings.start_url.clone();
                                metas = vec![TabMeta::new(&settings.start_url)];
                                pinned = 0;
                                cur_doc = None;
                                current_tab = 0;
                                let su = settings.start_url.clone();
                                let _ = ev!(&nav_target!(&su, false));
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
                                metas.push(TabMeta::new(&u));
                                tabs.push(u);
                                added = true;
                            }
                        }
                        if added {
                            let _ = ev!(&tabs_rt!());
                        }
                    }
                } else if msg == "next_tab" {
                    if tabs.len() > 1 {
                        current_tab = (current_tab + 1) % tabs.len();
                        cur_doc = None;
                        let target_url = tabs[current_tab].clone();
                        let label = titles.get(&target_url).cloned().unwrap_or_else(|| tab_label(&target_url));
                        window.set_title(&format!("{} — {}", label, app_title));
                        let _ = ev!(&nav_target!(&target_url, true));
                    }
                }
                // Закреплённые вкладки: показан ли WebView, который соответствует текущей вкладке. Нужно, когда вкладку
                // закрепили/открепили или переключили настройку: обычные переключения вкладок это уже сделали сами.
                if settings.live_pinned || !lives.is_empty() {
                    if !settings.live_pinned {
                        live_failed = None;
                    }
                    ensure_metas(&tabs, &mut metas);
                    let cur_tid = metas.get(current_tab).map(|m| m.id);
                    let want_live = settings.live_pinned
                        && current_tab < pinned
                        && cur_tid.is_some()
                        && live_failed != cur_tid;
                    if want_live != active_live.is_some() && current_tab < tabs.len() {
                        cur_doc = None;
                        let target = tabs[current_tab].clone();
                        if let Some(m) = metas.get_mut(current_tab) {
                            m.hist.in_step = true;
                        }
                        let shown = sel!();
                        if want_live && !shown {
                            live_failed = cur_tid;
                        }
                        let js = if shown { live_extra!() } else { nav_script(&target, true) };
                        let _ = ev!(&js);
                    }
                    live::prune(&webview, &mut lives, &mut active_live, &tabs, &metas, pinned, settings.live_pinned);
                    sync_mute!();
                }
                if msg != "GET_TABS" && msg != "GET_TABS|F" && !msg.starts_with("FAV|") && !msg.starts_with("WARM_TAB|") && msg != "SETTINGS_GET" && msg != "PICK_DL_DIR" {
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
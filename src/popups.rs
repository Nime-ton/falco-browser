//! Окна-попапы для window.open (вход через Google/Apple/GitHub, 3-D Secure) - только Windows.
//!
//! Обычный `window.open` с размерами окна WebView2 сообщает событием NewWindowRequested. Чтобы у попапа
//! работали `window.opener`, `postMessage` и `window.close()`, окно нужно создать как настоящий WebView2
//! из ТОЙ ЖЕ среды (Environment) и отдать его события через `SetNewWindow`. Окно создаётся асинхронно,
//! поэтому запрос откладывается (Deferral), а само окно делает главный цикл (событие "POPUP_REQ").
//!
//! Попап - обычное окно Windows с рамкой и заголовком "<сайт> - Falco" (адресной строки у него нет,
//! поэтому сайт всегда виден в заголовке). Панели вкладок и скриптов браузера в нём нет.

use std::cell::RefCell;
use std::rc::Rc;

use tao::dpi::LogicalSize;
use tao::event::WindowEvent;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder, WindowId};
use webview2_com::take_pwstr;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Deferral, ICoreWebView2Environment,
    ICoreWebView2NewWindowRequestedEventArgs, ICoreWebView2_2, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    WindowCloseRequestedEventHandler,
};
use windows::core::{ComInterface, PWSTR};
use windows::Win32::Foundation::{BOOL, E_POINTER, HWND, RECT};
use windows::Win32::System::WinRT::EventRegistrationToken;
use wry::WebViewExtWindows;

use crate::tabs::nav_allowed;
use crate::util::{host_of, is_http_url};

/// Сколько попапов может быть открыто одновременно.
pub const MAX_POPUPS: usize = 3;

const DEFAULT_W: u32 = 500;
const DEFAULT_H: u32 = 650;

/// Запрос на новое окно, ожидающий создания (его отложили через Deferral).
struct Pending {
    args: ICoreWebView2NewWindowRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
    env: ICoreWebView2Environment,
    width: u32,
    height: u32,
}

struct Popup {
    id: u32,
    window: Window,
    controller: Option<ICoreWebView2Controller>,
}

#[derive(Default)]
pub struct State {
    pending: Option<Pending>,
    popups: Vec<Popup>,
    next_id: u32,
}

pub type Shared = Rc<RefCell<State>>;

pub fn new_state() -> Shared {
    Rc::new(RefCell::new(State::default()))
}

/// Запрещаем окно: страница получит `null` из window.open.
unsafe fn deny(p: &Pending) {
    let _ = p.args.SetHandled(BOOL::from(true));
    let _ = p.deferral.Complete();
}

/// Подключает обработчик NewWindowRequested к главному WebView. Вместо `with_new_window_req_handler` из wry.
///
/// - окно с размером/без панелей (то, что делают окна входа и оплаты) -> настоящий попап;
/// - обычная ссылка target="_blank" -> как раньше, вкладка в фоне (событие "NEWWIN|<url>").
pub fn install(webview: &wry::WebView, state: Shared, proxy: EventLoopProxy<String>) -> windows::core::Result<()> {
    unsafe {
        let controller = webview.controller();
        let core = controller.CoreWebView2()?;
        let handler = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
            let args = match args {
                Some(a) => a,
                None => return Ok(()),
            };
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            let url = take_pwstr(uri);
            let mut user = BOOL::default();
            args.IsUserInitiated(&mut user)?;

            let mut has_size = BOOL::default();
            let mut toolbar = BOOL::from(true);
            let (mut w, mut h) = (0u32, 0u32);
            if let Ok(f) = args.WindowFeatures() {
                let _ = f.HasSize(&mut has_size);
                let _ = f.Width(&mut w);
                let _ = f.Height(&mut h);
                let _ = f.ShouldDisplayToolbar(&mut toolbar);
            }
            let popup = has_size.as_bool() || !toolbar.as_bool();

            if !popup {
                args.SetHandled(BOOL::from(true))?;
                let _ = proxy.send_event(format!("NEWWIN|{}", url));
                return Ok(());
            }

            // Попап: только по действию пользователя, только http(s) и about:blank
            // (about:blank нужен окнам оплаты: страница сама пишет в него форму 3-D Secure).
            let url_ok = url == "about:blank" || is_http_url(&url);
            let busy = match state.try_borrow() {
                Ok(s) => s.pending.is_some() || s.popups.len() >= MAX_POPUPS,
                Err(_) => true,
            };
            if !user.as_bool() || !url_ok || busy {
                args.SetHandled(BOOL::from(true))?;
                return Ok(());
            }
            let env = sender
                .and_then(|s| s.cast::<ICoreWebView2_2>().ok())
                .and_then(|s| s.Environment().ok());
            let env = match env {
                Some(e) => e,
                None => {
                    args.SetHandled(BOOL::from(true))?;
                    return Ok(());
                }
            };
            let deferral = match args.GetDeferral() {
                Ok(d) => d,
                Err(_) => {
                    args.SetHandled(BOOL::from(true))?;
                    return Ok(());
                }
            };
            let width = if w == 0 { DEFAULT_W } else { w.clamp(320, 1400) };
            let height = if h == 0 { DEFAULT_H } else { h.clamp(320, 1000) };
            state.borrow_mut().pending = Some(Pending { args, deferral, env, width, height });
            if proxy.send_event("POPUP_REQ".to_string()).is_err() {
                let p = state.borrow_mut().pending.take();
                if let Some(p) = p {
                    deny(&p);
                }
            }
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        core.add_NewWindowRequested(&handler, &mut token)?;
    }
    Ok(())
}

/// Вызывается главным циклом по событию "POPUP_REQ": создаёт окно и WebView2 в нём.
pub fn open_pending(
    elwt: &EventLoopWindowTarget<String>,
    owner: &Window,
    state: &Shared,
    proxy: &EventLoopProxy<String>,
) {
    let pending = match state.borrow_mut().pending.take() {
        Some(p) => p,
        None => return,
    };
    if state.borrow().popups.len() >= MAX_POPUPS {
        unsafe { deny(&pending) };
        return;
    }
    // owner: попап всегда над окном браузера и сворачивается вместе с ним
    let window = WindowBuilder::new()
        .with_title("Falco")
        .with_inner_size(LogicalSize::new(pending.width as f64, pending.height as f64))
        .with_owner_window(owner.hwnd())
        .build(elwt);
    let window = match window {
        Ok(w) => w,
        Err(_) => {
            unsafe { deny(&pending) };
            return;
        }
    };
    let hwnd = HWND(window.hwnd() as isize);
    let size = window.inner_size();
    let id = {
        let mut st = state.borrow_mut();
        st.next_id += 1;
        let id = st.next_id;
        st.popups.push(Popup { id, window, controller: None });
        id
    };

    let env = pending.env.clone();
    let cell = Rc::new(RefCell::new(Some(pending)));
    let cell_cb = cell.clone();
    let state_cb = state.clone();
    let proxy_cb = proxy.clone();
    let (bw, bh) = (size.width as i32, size.height as i32);
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |res, controller| {
        let p = cell_cb.borrow_mut().take();
        if let Some(p) = p {
            let done = unsafe { finish(&state_cb, &proxy_cb, id, (bw, bh), &p, res, controller) };
            if done.is_err() {
                unsafe { deny(&p) };
                close_where(&state_cb, |x| x.id == id);
            }
        }
        Ok(())
    }));
    let started = unsafe { env.CreateCoreWebView2Controller(hwnd, &handler) };
    if started.is_err() {
        let p = cell.borrow_mut().take();
        if let Some(p) = p {
            unsafe { deny(&p) };
        }
        close_where(state, |x| x.id == id);
    }
}

/// Контроллер готов: настраиваем окно и отдаём его странице-открывателю.
unsafe fn finish(
    state: &Shared,
    proxy: &EventLoopProxy<String>,
    id: u32,
    bounds: (i32, i32),
    p: &Pending,
    res: windows::core::Result<()>,
    controller: Option<ICoreWebView2Controller>,
) -> windows::core::Result<()> {
    res?;
    let controller = controller.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
    let core = controller.CoreWebView2()?;
    controller.SetBounds(RECT { left: 0, top: 0, right: bounds.0, bottom: bounds.1 })?;
    controller.SetIsVisible(BOOL::from(true))?;
    if let Ok(settings) = core.Settings() {
        let _ = settings.SetAreDevToolsEnabled(BOOL::from(false));
    }

    // 1. Навигация: только http(s)/about/blob; заголовок окна = сайт (адресной строки у попапа нет)
    let st_nav = state.clone();
    let nav = NavigationStartingEventHandler::create(Box::new(move |_, a| {
        if let Some(a) = a {
            let mut uri = PWSTR::null();
            a.Uri(&mut uri)?;
            let url = take_pwstr(uri);
            if !nav_allowed(&url) {
                a.SetCancel(BOOL::from(true))?;
            } else if let Some(host) = host_of(&url) {
                let title = if url.starts_with("http://") {
                    format!("{} (не защищено) — Falco", host)
                } else {
                    format!("{} — Falco", host)
                };
                if let Ok(st) = st_nav.try_borrow() {
                    if let Some(pp) = st.popups.iter().find(|x| x.id == id) {
                        pp.window.set_title(&title);
                    }
                }
            }
        }
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    core.add_NavigationStarting(&nav, &mut token)?;

    // 2. window.close() из попапа -> закрыть окно (делает главный цикл)
    let proxy_close = proxy.clone();
    let close = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
        let _ = proxy_close.send_event(format!("POPUP_CLOSE|{}", id));
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    core.add_WindowCloseRequested(&close, &mut token)?;

    // 3. Попап не открывает новых окон (цепочки попапов - частый приём рекламы)
    let nw = NewWindowRequestedEventHandler::create(Box::new(move |_, a| {
        if let Some(a) = a {
            a.SetHandled(BOOL::from(true))?;
        }
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    core.add_NewWindowRequested(&nw, &mut token)?;

    // 4. Отдаём окно открывателю. После Complete() window.open() на странице вернёт живой объект окна.
    p.args.SetNewWindow(&core)?;
    p.args.SetHandled(BOOL::from(true))?;
    p.deferral.Complete()?;

    let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    if let Ok(mut st) = state.try_borrow_mut() {
        if let Some(pp) = st.popups.iter_mut().find(|x| x.id == id) {
            pp.controller = Some(controller);
        }
    }
    Ok(())
}

fn close_where(state: &Shared, pred: impl Fn(&Popup) -> bool) {
    let popup = {
        let mut st = match state.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        let pos = st.popups.iter().position(|p| pred(p));
        pos.map(|i| st.popups.remove(i))
    };
    if let Some(p) = popup {
        if let Some(c) = &p.controller {
            unsafe {
                let _ = c.Close();
            }
        }
        // окно уничтожается, когда `p` выходит из области видимости
    }
}

/// "POPUP_CLOSE|<id>": страница попапа вызвала window.close().
pub fn close_id(state: &Shared, id: u32) {
    close_where(state, |p| p.id == id);
}

fn controller_of(state: &Shared, wid: WindowId) -> Option<ICoreWebView2Controller> {
    let st = state.try_borrow().ok()?;
    st.popups.iter().find(|p| p.window.id() == wid)?.controller.clone()
}

/// События окон, не относящихся к главному: закрытие, размер, перемещение, фокус.
pub fn on_window_event(state: &Shared, wid: WindowId, ev: &WindowEvent<'_>) {
    match ev {
        WindowEvent::CloseRequested => close_where(state, |p| p.window.id() == wid),
        WindowEvent::Resized(size) => {
            if let Some(c) = controller_of(state, wid) {
                unsafe {
                    let _ = c.SetBounds(RECT { left: 0, top: 0, right: size.width as i32, bottom: size.height as i32 });
                }
            }
        }
        WindowEvent::Moved(_) => {
            if let Some(c) = controller_of(state, wid) {
                unsafe {
                    let _ = c.NotifyParentWindowPositionChanged();
                }
            }
        }
        WindowEvent::Focused(true) => {
            if let Some(c) = controller_of(state, wid) {
                unsafe {
                    let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                }
            }
        }
        _ => {}
    }
}
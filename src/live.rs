//! "Живые" закреплённые вкладки.
//!
//! Обычные вкладки Falco - это один общий WebView, который переходит с адреса на адрес (так экономнее всего).
//! Закреплённая вкладка при включённой настройке `live_pinned` получает СВОЙ WebView: страница в нём не
//! выгружается при переключении на другие вкладки, поэтому музыка, видео и звонки продолжают играть в фоне.
//!
//! - WebView закреплённой вкладки создаётся лениво: при первом переходе на неё (или сразу при закреплении
//!   текущей вкладки). Пока вы её не открывали, она ничего не потребляет.
//! - Скрытый WebView стоит за краем окна и невидим (`SetIsVisible(false)`) - страница остаётся живой.
//! - Все WebView используют один профиль (куки, вход в аккаунты общие).
//! - Связь вкладки с WebView идёт по уникальному `tab_id` (`TabMeta::id`), поэтому даже две закреплённые
//!   вкладки с одинаковым адресом работают независимо друг от друга.
//! - Когда вкладку открепили/закрыли/выключили настройку, её WebView закрывается (`prune`).
//! - Сообщения от страницы такого WebView приходят в главный цикл с меткой "LV<id>~": по ней видно, от какого
//!   WebView сообщение, и фоновые страницы не путают состояние активной вкладки.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tao::event_loop::EventLoopProxy;
use tao::window::Window;
use wry::{Rect, WebContext, WebView, WebViewBuilder};
#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

use crate::config::MAX_IPC_LEN;
#[cfg(not(windows))]
use crate::downloads::choose_download_path;
use crate::tabs::{nav_allowed, TabMeta};
use crate::util::is_http_url;

/// Закреплённая вкладка со своим WebView.
pub(crate) struct Live {
    pub(crate) id: u32,
    /// Уникальный идентификатор вкладки (`TabMeta::id`), к которой привязан этот WebView.
    pub(crate) tab_id: u64,
    /// Текущий адрес страницы в этом WebView.
    pub(crate) url: String,
    /// id документа, который сейчас живёт в этом WebView (см. `filter_doc_msg` в tabs.rs).
    pub(crate) doc: Option<String>,
    /// Проигрывает ли этот WebView звук прямо сейчас.
    pub(crate) playing: bool,
    #[cfg(all(windows, feature = "native-adblock"))]
    pub(crate) page_url: std::rc::Rc<std::cell::RefCell<String>>,
    pub(crate) view: WebView,
}

/// Всё, что нужно для создания ещё одного WebView с теми же настройками, что и у главного.
pub(crate) struct Factory {
    pub(crate) proxy: EventLoopProxy<String>,
    pub(crate) ipc_prefix: String,
    /// Скрипты инициализации в том же порядке, что у главного WebView (косметика, браузерный скрипт, пользовательские).
    pub(crate) scripts: Vec<String>,
    /// Аргументы браузера WebView2. Должны совпадать с главным WebView, иначе WebView2 не создастся.
    pub(crate) browser_args: String,
    pub(crate) dl_dir: Arc<Mutex<String>>,
    /// Спрашивать, куда сохранить файл (общий флаг с главным WebView) и окно-владелец диалога
    pub(crate) ask_dl: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) hwnd: isize,
    /// Путь последней загрузки: нужен только запасному обработчику wry (не Windows); на Windows загрузки ведёт downloads.rs
    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) last_dl: Arc<Mutex<PathBuf>>,
    pub(crate) dl_mgr: Arc<Mutex<crate::downloads::DownloadManager>>,
    pub(crate) ctx: WebContext,
    pub(crate) next_id: u32,
    #[cfg(windows)]
    pub(crate) trusted: Vec<String>,
    #[cfg(windows)]
    pub(crate) media_mode: Arc<std::sync::atomic::AtomicU8>,
    #[cfg(windows)]
    pub(crate) popups: crate::popups::Shared,
    #[cfg(all(windows, feature = "native-adblock"))]
    pub(crate) adblock: crate::native_adblock::Checker,
    #[cfg(all(windows, feature = "native-adblock"))]
    pub(crate) whitelist: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    #[cfg(all(windows, feature = "native-adblock"))]
    pub(crate) adblock_log: PathBuf,
}

fn full_rect(window: &Window) -> Rect {
    let s = window.inner_size();
    Rect { x: 0, y: 0, width: s.width, height: s.height }
}

/// Скрытый WebView паркуется за левым краем окна: не виден и не перехватывает мышь.
fn parked_rect(window: &Window) -> Rect {
    let s = window.inner_size();
    Rect { x: -30000, y: 0, width: s.width, height: s.height }
}

/// Показать/скрыть WebView (у показанного забираем фокус клавиатуры, чтобы работали горячие клавиши).
#[cfg(windows)]
pub(crate) fn set_visible(view: &WebView, visible: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
    use windows::Win32::Foundation::BOOL;
    use wry::WebViewExtWindows;
    unsafe {
        let controller = view.controller();
        let _ = controller.SetIsVisible(BOOL::from(visible));
        if visible {
            let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn set_visible(_view: &WebView, _visible: bool) {}

impl Live {
    pub(crate) fn show(&self, window: &Window) {
        let _ = self.view.set_bounds(full_rect(window));
        set_visible(&self.view, true);
    }

    pub(crate) fn hide(&self, window: &Window) {
        set_visible(&self.view, false);
        let _ = self.view.set_bounds(parked_rect(window));
    }
}

impl Factory {
    /// Создаёт WebView для закреплённой вкладки `tab_id` с адресом `url` (он сразу занимает всё окно).
    pub(crate) fn build(&mut self, window: &Window, tab_id: u64, url: &str) -> Result<Live, String> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let tag = format!("LV{}~", id);
        let prefix = self.ipc_prefix.clone();
        let proxy_ipc = self.proxy.clone();

        let mut builder = WebViewBuilder::new_as_child(window)
            .with_bounds(full_rect(window))
            .with_background_color((32, 33, 36, 255))
            .with_hotkeys_zoom(false)
            .with_autoplay(true)
            .with_devtools(false)
            .with_web_context(&mut self.ctx);
        for s in &self.scripts {
            builder = builder.with_initialization_script(s);
        }
        let builder = builder
            .with_ipc_handler(move |msg: String| {
                if msg.len() > MAX_IPC_LEN {
                    return;
                }
                if let Some(rest) = msg.strip_prefix(prefix.as_str()) {
                    let _ = proxy_ipc.send_event(format!("{}{}", tag, rest));
                }
            })
            .with_navigation_handler(|u: String| nav_allowed(&u));

        // На не-Windows используем стандартный обработчик wry (на Windows ставится нативный с прогрессом и паузой)
        #[cfg(not(windows))]
        let builder = {
            let proxy_dl_start = self.proxy.clone();
            let proxy_dl_done = self.proxy.clone();
            let dl_dir = self.dl_dir.clone();
            let ask_dl = self.ask_dl.clone();
            let owner_hwnd = self.hwnd;
            let last_for_start = self.last_dl.clone();
            let last_for_done = self.last_dl.clone();
            builder
                .with_download_started_handler(move |dl_url: String, path: &mut PathBuf| {
                    if dl_url.starts_with("file:") {
                        return false;
                    }
                    let custom = dl_dir.lock().ok().map(|g| g.clone()).unwrap_or_default();
                    let ask = ask_dl.load(std::sync::atomic::Ordering::Relaxed);
                    let dest = match choose_download_path(path, &custom, ask, owner_hwnd) {
                        Some(d) => d,
                        None => return false,
                    };
                    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("файл").to_string();
                    let dest_str = dest.to_string_lossy().into_owned();
                    if let Ok(mut guard) = last_for_start.lock() {
                        *guard = dest.clone();
                    }
                    *path = dest;
                    let _ = proxy_dl_start.send_event(format!("DL_START|0|{}|{}|{}|0", name, dest_str, dl_url));
                    true
                })
                .with_download_completed_handler(move |dl_url, path, success| {
                    let resolved = path.or_else(|| {
                        last_for_done
                            .lock()
                            .ok()
                            .map(|g| g.clone())
                            .filter(|p| !p.as_os_str().is_empty())
                    });
                    let full_path = resolved.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
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

        let builder = builder.with_url(url).map_err(|e| format!("{:?}", e))?;

        #[cfg(windows)]
        let builder = builder.with_additional_browser_args(self.browser_args.clone());

        let view = builder.build().map_err(|e| format!("{:?}", e))?;
        let _ = view.zoom(1.0);

        #[cfg(all(windows, feature = "native-adblock"))]
        let page_url = std::rc::Rc::new(std::cell::RefCell::new(url.to_string()));

        // то же, что и у главного WebView: разрешения камеры/микрофона, звук, попапы, загрузки и нативный блокировщик
        #[cfg(windows)]
        {
            let _ = crate::media::permissions::install(&view, self.trusted.clone(), self.media_mode.clone());
            let _ = crate::media::audio::install(&view, self.proxy.clone(), Some(id));
            let _ = crate::popups::install(&view, self.popups.clone(), self.proxy.clone());
            let _ = crate::downloads::install_webview_downloads(
                &view,
                self.proxy.clone(),
                self.dl_dir.clone(),
                self.ask_dl.clone(),
                self.hwnd,
                self.dl_mgr.clone(),
            );
        }
        #[cfg(all(windows, feature = "native-adblock"))]
        {
            let _ = crate::native_adblock::install(
                &view,
                self.adblock.clone(),
                page_url.clone(),
                self.whitelist.clone(),
                self.adblock_log.clone(),
            );
        }

        Ok(Live {
            id,
            tab_id,
            url: url.to_string(),
            doc: None,
            playing: false,
            #[cfg(all(windows, feature = "native-adblock"))]
            page_url,
            view,
        })
    }
}

/// "LV<id>~<сообщение>" -> (id, сообщение).
pub(crate) fn split_tag(msg: &str) -> Option<(u32, &str)> {
    let rest = msg.strip_prefix("LV")?;
    let (id, inner) = rest.split_once('~')?;
    Some((id.parse::<u32>().ok()?, inner))
}

/// Сообщения страницы, которые привязаны к документу. От главного (скрытого) WebView они не нужны,
/// пока показана закреплённая вкладка: иначе чужой документ подменит текущий.
pub(crate) fn is_page_doc_msg(msg: &str) -> bool {
    msg.starts_with("LOAD_URL|") || msg.starts_with("SYNC_URL|") || msg.starts_with("TITLE|")
}

fn apply_bg(
    doc: &mut Option<String>,
    url: &mut String,
    tab_id: u64,
    tabs: &mut [String],
    metas: &mut [TabMeta],
    pinned: usize,
    inner: &str,
) -> bool {
    let (is_load, rest) = if let Some(r) = inner.strip_prefix("LOAD_URL|") {
        (true, r)
    } else if let Some(r) = inner.strip_prefix("SYNC_URL|") {
        (false, r)
    } else {
        return false;
    };
    let (d, u) = match rest.split_once('|') {
        Some(x) => x,
        None => return false,
    };
    if d.is_empty() || d.len() > 40 || !d.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    if is_load {
        *doc = Some(d.to_string());
    } else if doc.as_deref() != Some(d) {
        return false;
    }
    if !is_http_url(u) {
        return false;
    }
    let n = pinned.min(tabs.len()).min(metas.len());
    if let Some(i) = metas[..n].iter().position(|m| m.id == tab_id) {
        tabs[i] = u.to_string();
        metas[i].hist.record(u);
        *url = u.to_string();
        true
    } else {
        false
    }
}

/// Сообщение от фонового (не показанного) закреплённого WebView. Из него берём только адрес и id документа:
/// адрес нужен, чтобы вкладка сохранялась в сессии с актуальной страницей (например, следующее видео).
pub(crate) fn background_msg(
    lives: &mut [Live],
    tabs: &mut [String],
    metas: &mut [TabMeta],
    pinned: usize,
    id: u32,
    inner: &str,
) {
    if let Some(l) = lives.iter_mut().find(|l| l.id == id) {
        if apply_bg(&mut l.doc, &mut l.url, l.tab_id, tabs, metas, pinned, inner) {
            #[cfg(all(windows, feature = "native-adblock"))]
            {
                *l.page_url.borrow_mut() = l.url.clone();
            }
        }
    }
}

/// Показывает WebView, который соответствует вкладке `idx`. Возвращает true, если показан закреплённый WebView
/// (тогда страницу переключать не нужно - она уже живая), false - если показан общий WebView
/// (тогда вызывающий код переводит его на адрес вкладки, как раньше).
pub(crate) fn select(
    window: &Window,
    main: &WebView,
    factory: &mut Factory,
    lives: &mut Vec<Live>,
    active: &mut Option<u32>,
    cur_doc: &mut Option<String>,
    tabs: &[String],
    metas: &[TabMeta],
    pinned: usize,
    idx: usize,
    enabled: bool,
) -> bool {
    if enabled && idx < pinned && idx < tabs.len() && idx < metas.len() {
        let tab_id = metas[idx].id;
        let url = tabs[idx].as_str();
        let pos = match lives.iter().position(|l| l.tab_id == tab_id) {
            Some(p) => Some(p),
            None => match factory.build(window, tab_id, url) {
                Ok(l) => {
                    lives.push(l);
                    Some(lives.len() - 1)
                }
                Err(_) => None,
            },
        };
        if let Some(p) = pos {
            let new_id = lives[p].id;
            if *active != Some(new_id) {
                match *active {
                    Some(old) => {
                        if let Some(l) = lives.iter().find(|l| l.id == old) {
                            l.hide(window);
                        }
                    }
                    None => {
                        set_visible(main, false);
                        // Освобождаем общий WebView, чтобы он не дублировал звук в фоне после закрепления вкладки
                        let _ = main.evaluate_script(
                            "try{if(window.__ulSaveForSwitch)window.__ulSaveForSwitch();window.location.replace('about:blank');}catch(e){}",
                        );
                    }
                }
                lives[p].show(window);
                *active = Some(new_id);
            }
            *cur_doc = lives[p].doc.clone();
            return true;
        }
    }
    // обычная вкладка (или живой WebView создать не удалось): показываем общий WebView
    if let Some(old) = active.take() {
        if let Some(l) = lives.iter().find(|l| l.id == old) {
            l.hide(window);
        }
        set_visible(main, true);
    }
    false
}

/// Закрывает WebView закреплённых вкладок, которых больше нет (открепили, закрыли) или все - если настройку выключили.
pub(crate) fn prune(
    main: &WebView,
    lives: &mut Vec<Live>,
    active: &mut Option<u32>,
    tabs: &[String],
    metas: &[TabMeta],
    pinned: usize,
    enabled: bool,
) {
    if lives.is_empty() {
        return;
    }
    let n = pinned.min(tabs.len()).min(metas.len());
    lives.retain(|l| enabled && metas[..n].iter().any(|m| m.id == l.tab_id));
    if let Some(id) = *active {
        if !lives.iter().any(|l| l.id == id) {
            *active = None;
            set_visible(main, true);
        }
    }
}

/// Окно изменило размер: показанный WebView растягиваем, скрытые остаются за краем.
pub(crate) fn on_resize(window: &Window, lives: &[Live], active: Option<u32>) {
    for l in lives {
        let r = if Some(l.id) == active { full_rect(window) } else { parked_rect(window) };
        let _ = l.view.set_bounds(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> (Vec<String>, Vec<TabMeta>) {
        let tabs: Vec<String> = xs.iter().map(|s| s.to_string()).collect();
        let metas: Vec<TabMeta> = tabs.iter().map(|u| TabMeta::new(u)).collect();
        (tabs, metas)
    }

    #[test]
    fn tag_is_split() {
        assert_eq!(split_tag("LV3~SYNC_URL|abc|https://a.com/"), Some((3, "SYNC_URL|abc|https://a.com/")));
        assert_eq!(split_tag("GET_TABS"), None);
        assert_eq!(split_tag("LVx~a"), None);
        assert_eq!(split_tag("LV7"), None);
    }

    #[test]
    fn page_doc_messages_are_recognized() {
        assert!(is_page_doc_msg("LOAD_URL|a|https://a.com/"));
        assert!(is_page_doc_msg("TITLE|a|x"));
        assert!(!is_page_doc_msg("GET_TABS"));
    }

    #[test]
    fn background_page_updates_its_pinned_tab_by_id_even_with_duplicate_urls() {
        let (mut tabs, mut metas) = v(&["https://yt.com/same", "https://yt.com/same", "https://c.com/"]);
        let second_id = metas[1].id;
        let mut doc = None;
        let mut url = "https://yt.com/same".to_string();
        assert!(apply_bg(&mut doc, &mut url, second_id, &mut tabs, &mut metas, 2, "LOAD_URL|d1|https://yt.com/same"));
        assert_eq!(doc.as_deref(), Some("d1"));
        assert!(apply_bg(&mut doc, &mut url, second_id, &mut tabs, &mut metas, 2, "SYNC_URL|d1|https://yt.com/next"));
        // Первая вкладка с тем же начальным адресом осталась нетронутой, обновилась именно вторая
        assert_eq!(tabs[0], "https://yt.com/same");
        assert_eq!(tabs[1], "https://yt.com/next");
        assert_eq!(url, "https://yt.com/next");
        assert!(metas[1].hist.can_back());
    }

    #[test]
    fn background_stale_or_foreign_messages_are_ignored() {
        let (mut tabs, mut metas) = v(&["https://yt.com/1", "https://c.com/"]);
        let tid = metas[0].id;
        let mut doc = Some("d1".to_string());
        let mut url = "https://yt.com/1".to_string();
        assert!(!apply_bg(&mut doc, &mut url, tid, &mut tabs, &mut metas, 1, "SYNC_URL|old|https://evil.com/"));
        assert!(!apply_bg(&mut doc, &mut url, tid, &mut tabs, &mut metas, 1, "SYNC_URL|d1|javascript:alert(1)"));
        assert!(!apply_bg(&mut doc, &mut url, tid, &mut tabs, &mut metas, 1, "TITLE|d1|hello"));
        assert_eq!(tabs[0], "https://yt.com/1");
        assert_eq!(url, "https://yt.com/1");
    }

    #[test]
    fn background_page_outside_pinned_block_does_not_touch_regular_tabs() {
        let (mut tabs, mut metas) = v(&["https://a.com/", "https://yt.com/1"]);
        let regular_tid = metas[1].id;
        let mut doc = Some("d1".to_string());
        let mut url = "https://yt.com/1".to_string();
        assert!(!apply_bg(&mut doc, &mut url, regular_tid, &mut tabs, &mut metas, 1, "SYNC_URL|d1|https://yt.com/2"));
        assert_eq!(tabs[1], "https://yt.com/1");
    }
}
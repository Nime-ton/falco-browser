//! Загрузки: нативный прогресс (ICoreWebView2_4), пауза/возобновление/отмена, выбор папки, Drag & Drop файла и контекстные операции.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use tao::event_loop::EventLoopProxy;

use crate::config::*;
use crate::history::*;
use crate::util::*;

pub(crate) const DOWNLOADS_MAX: usize = 500;

static NEXT_DL_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Clone)]
pub(crate) struct DlItem {
    pub(crate) ts: u64,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) url: String,
}

#[derive(Default)]
pub(crate) struct DownloadManager {
    #[cfg(windows)]
    ops: HashMap<u32, webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation>,
}

impl DownloadManager {
    #[cfg(windows)]
    pub(crate) fn insert(
        &mut self,
        id: u32,
        op: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation,
    ) {
        self.ops.insert(id, op);
    }

    #[cfg(windows)]
    pub(crate) fn remove(&mut self, id: u32) {
        self.ops.remove(&id);
    }
    #[cfg(not(windows))]
    pub(crate) fn remove(&mut self, _id: u32) {}

    pub(crate) fn pause(&self, id: u32) {
        #[cfg(windows)]
        if let Some(op) = self.ops.get(&id) {
            unsafe {
                let _ = op.Pause();
            }
        }
        #[cfg(not(windows))]
        let _ = id;
    }

    pub(crate) fn resume(&self, id: u32) {
        #[cfg(windows)]
        if let Some(op) = self.ops.get(&id) {
            unsafe {
                let _ = op.Resume();
            }
        }
        #[cfg(not(windows))]
        let _ = id;
    }

    pub(crate) fn cancel(&self, id: u32) {
        #[cfg(windows)]
        if let Some(op) = self.ops.get(&id) {
            unsafe {
                let _ = op.Cancel();
            }
        }
        #[cfg(not(windows))]
        let _ = id;
    }
}

/// Устанавливает нативный обработчик загрузок WebView2 с поддержкой прогресса, паузы и отмены.
#[cfg(windows)]
pub(crate) fn install_webview_downloads(
    webview: &wry::WebView,
    proxy: EventLoopProxy<String>,
    dl_dir: Arc<Mutex<String>>,
    ask_dl: Arc<AtomicBool>,
    hwnd: isize,
    manager: Arc<Mutex<DownloadManager>>,
) -> windows::core::Result<()> {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_4, COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED,
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
    };
    use webview2_com::{
        BytesReceivedChangedEventHandler, DownloadStartingEventHandler, StateChangedEventHandler,
        take_pwstr,
    };
    use windows::core::{ComInterface, HSTRING, PWSTR};
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::WinRT::EventRegistrationToken;
    use wry::WebViewExtWindows;

    unsafe {
        let controller = webview.controller();
        let core = controller.CoreWebView2()?;
        let core4: ICoreWebView2_4 = core.cast()?;

        let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
            let args = match args {
                Some(a) => a,
                None => return Ok(()),
            };
            let op = args.DownloadOperation()?;

            let mut uri_pw = PWSTR::null();
            op.Uri(&mut uri_pw)?;
            let url = take_pwstr(uri_pw);
            if url.starts_with("file:") {
                args.SetCancel(BOOL::from(true))?;
                return Ok(());
            }

            let mut path_pw = PWSTR::null();
            args.ResultFilePath(&mut path_pw)?;
            let proposed = PathBuf::from(take_pwstr(path_pw));

            let custom = dl_dir.lock().ok().map(|g| g.clone()).unwrap_or_default();
            let ask = ask_dl.load(Ordering::Relaxed);
            let dest = match choose_download_path(&proposed, &custom, ask, hwnd) {
                Some(d) => d,
                None => {
                    args.SetCancel(BOOL::from(true))?;
                    return Ok(());
                }
            };

            let dest_str = dest.to_string_lossy().into_owned();
            args.SetResultFilePath(&HSTRING::from(dest_str.as_str()))?;
            args.SetHandled(BOOL::from(true))?;

            let name = dest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("файл")
                .to_string();

            let mut total: i64 = 0;
            let _ = op.TotalBytesToReceive(&mut total);

            let id = NEXT_DL_ID.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut mgr) = manager.lock() {
                mgr.insert(id, op.clone());
            }

            let _ = proxy.send_event(format!(
                "DL_START|{}|{}|{}|{}|{}",
                id, name, dest_str, url, total
            ));

            // 1. Отслеживание прогресса и скорости (не чаще 5 раз в секунду, чтобы не нагружать UI)
            let proxy_prog = proxy.clone();
            let name_prog = name.clone();
            let path_prog = dest_str.clone();
            let url_prog = url.clone();
            let last_tick = Rc::new(Cell::new(Instant::now() - Duration::from_secs(1)));
            let last_bytes = Rc::new(Cell::new(0i64));
            let speed_ema = Rc::new(Cell::new(0i64));

            let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
                let op = match sender {
                    Some(s) => s,
                    None => return Ok(()),
                };
                let now = Instant::now();
                let prev_t = last_tick.get();
                let elapsed = now.duration_since(prev_t).as_secs_f64();

                let mut rec: i64 = 0;
                let mut tot: i64 = 0;
                let _ = op.BytesReceived(&mut rec);
                let _ = op.TotalBytesToReceive(&mut tot);

                if elapsed < 0.18 && (tot <= 0 || rec < tot) {
                    return Ok(());
                }

                let prev_b = last_bytes.get();
                let instant_speed = if elapsed > 0.02 && rec >= prev_b {
                    ((rec - prev_b) as f64 / elapsed) as i64
                } else {
                    0
                };
                let old_s = speed_ema.get();
                let spd = if old_s == 0 {
                    instant_speed
                } else {
                    (old_s * 3 + instant_speed * 2) / 5
                };

                last_tick.set(now);
                last_bytes.set(rec);
                speed_ema.set(spd);

                let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
                let _ = op.InterruptReason(&mut reason);
                let paused = if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
                    "1"
                } else {
                    "0"
                };

                let _ = proxy_prog.send_event(format!(
                    "DL_PROG|{}|{}|{}|{}|{}|{}|{}|{}",
                    id, rec, tot, spd, paused, name_prog, path_prog, url_prog
                ));
                Ok(())
            }));
            let mut b_tok = EventRegistrationToken::default();
            let _ = op.add_BytesReceivedChanged(&bytes_handler, &mut b_tok);

            // 2. Отслеживание паузы, возобновления, завершения и отмены
            let proxy_state = proxy.clone();
            let name_state = name;
            let path_state = dest_str;
            let url_state = url;
            let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _| {
                let op = match sender {
                    Some(s) => s,
                    None => return Ok(()),
                };
                let mut st = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
                let _ = op.State(&mut st);
                let mut rec: i64 = 0;
                let mut tot: i64 = 0;
                let _ = op.BytesReceived(&mut rec);
                let _ = op.TotalBytesToReceive(&mut tot);

                if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                    let _ = proxy_state.send_event(format!(
                        "DL_DONE|{}|ok|{}|{}|{}|{}",
                        id, name_state, path_state, url_state, tot.max(rec)
                    ));
                } else if st == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
                    let _ = op.InterruptReason(&mut reason);
                    if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
                        let _ = proxy_state.send_event(format!(
                            "DL_PROG|{}|{}|{}|0|1|{}|{}|{}",
                            id, rec, tot, name_state, path_state, url_state
                        ));
                    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
                        let _ = proxy_state.send_event(format!(
                            "DL_DONE|{}|cancel|{}|{}|{}|{}",
                            id, name_state, path_state, url_state, rec
                        ));
                    } else {
                        let _ = proxy_state.send_event(format!(
                            "DL_DONE|{}|err|{}|{}|{}|{}",
                            id, name_state, path_state, url_state, rec
                        ));
                    }
                } else if st == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                    let _ = proxy_state.send_event(format!(
                        "DL_PROG|{}|{}|{}|0|0|{}|{}|{}",
                        id, rec, tot, name_state, path_state, url_state
                    ));
                }
                Ok(())
            }));
            let mut s_tok = EventRegistrationToken::default();
            let _ = op.add_StateChanged(&state_handler, &mut s_tok);

            Ok(())
        }));

        let mut token = EventRegistrationToken::default();
        core4.add_DownloadStarting(&handler, &mut token)?;
    }
    Ok(())
}

// ---------- Системный OLE Drag & Drop скачанного файла (в мессенджеры, почту, Проводник) ----------

#[cfg(windows)]
mod ole_drag {
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::core::{Interface, GUID, HRESULT, HSTRING};
    use windows::Win32::Foundation::{
        BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, E_NOINTERFACE,
        E_POINTER, S_OK,
    };
    use windows::Win32::System::Com::{IBindCtx, IDataObject};
    use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName};

    const MK_LBUTTON: u32 = 1;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_LINK: u32 = 4;

    const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
    const IID_IDROPSOURCE: GUID = GUID::from_u128(0x00000121_0000_0000_c000_000000000046);

    #[link(name = "ole32")]
    extern "system" {
        fn DoDragDrop(
            pdataobj: *mut c_void,
            pdropsource: *mut c_void,
            dwokeffects: u32,
            pdweffect: *mut u32,
        ) -> HRESULT;
        fn ReleaseCapture() -> BOOL;
    }

    #[repr(C)]
    struct IDropSourceVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        query_continue_drag: unsafe extern "system" fn(*mut c_void, BOOL, u32) -> HRESULT,
        give_feedback: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    }

    #[repr(C)]
    struct DropSource {
        vtbl: *const IDropSourceVtbl,
        refs: AtomicU32,
    }

    static VTBL: IDropSourceVtbl = IDropSourceVtbl {
        query_interface: ds_qi,
        add_ref: ds_add_ref,
        release: ds_release,
        query_continue_drag: ds_query_continue,
        give_feedback: ds_give_feedback,
    };

    unsafe extern "system" fn ds_qi(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if ppv.is_null() {
            return E_POINTER;
        }
        let iid = &*riid;
        if *iid == IID_IUNKNOWN || *iid == IID_IDROPSOURCE {
            *ppv = this;
            ds_add_ref(this);
            S_OK
        } else {
            *ppv = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn ds_add_ref(this: *mut c_void) -> u32 {
        let me = &*(this as *const DropSource);
        me.refs.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn ds_release(this: *mut c_void) -> u32 {
        let me = &*(this as *const DropSource);
        let prev = me.refs.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut DropSource));
        }
        prev - 1
    }

    unsafe extern "system" fn ds_query_continue(
        _this: *mut c_void,
        escape_pressed: BOOL,
        key_state: u32,
    ) -> HRESULT {
        if escape_pressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (key_state & MK_LBUTTON) == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    unsafe extern "system" fn ds_give_feedback(_this: *mut c_void, _effect: u32) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }

    pub fn run(path: &Path) {
        if !path.exists() || !path.is_file() {
            return;
        }
        let full = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let s = full.to_string_lossy();
        let clean = s.strip_prefix(r"\\?\").unwrap_or(&s);

        unsafe {
            let item: IShellItem =
                match SHCreateItemFromParsingName(&HSTRING::from(clean), None::<&IBindCtx>) {
                    Ok(i) => i,
                    Err(_) => return,
                };
            let data_obj: IDataObject = match item.BindToHandler(None::<&IBindCtx>, &BHID_DataObject) {
                Ok(d) => d,
                Err(_) => return,
            };

            let src = Box::into_raw(Box::new(DropSource {
                vtbl: &VTBL,
                refs: AtomicU32::new(1),
            }));

            // Освобождаем возможный захват мыши внутри окна перед стартом системного цикла перетаскивания
            let _ = ReleaseCapture();

            let mut effect: u32 = 0;
            let _ = DoDragDrop(
                data_obj.as_raw(),
                src as *mut c_void,
                DROPEFFECT_COPY | DROPEFFECT_LINK,
                &mut effect,
            );
            ds_release(src as *mut c_void);
        }
    }
}

/// Запускает нативное перетаскивание скачанного файла мышкой (работает и внутри браузера, и в любые внешние программы).
pub(crate) fn start_file_drag(file_path: &str) {
    let p = Path::new(file_path);
    #[cfg(windows)]
    ole_drag::run(p);
    #[cfg(not(windows))]
    let _ = p;
}

pub(crate) fn default_download_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Downloads"))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Downloads")))
        .unwrap_or_else(std::env::temp_dir)
}

pub(crate) fn resolve_download_dir(custom_dir: &str) -> PathBuf {
    let trimmed = custom_dir.trim();
    if !trimmed.is_empty() {
        return PathBuf::from(trimmed);
    }
    default_download_dir()
}

pub(crate) fn pick_download_folder(window: &tao::window::Window) -> Option<String> {
    #[cfg(windows)]
    unsafe {
        use tao::platform::windows::WindowExtWindows;
        use webview2_com::take_pwstr;
        use windows::core::w;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{
            FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
        };

        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().unwrap_or_default();
        dialog.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let _ = dialog.SetTitle(w!("Выберите папку для загрузок"));
        if dialog.Show(HWND(window.hwnd() as isize)).is_err() {
            return None;
        }
        let item = dialog.GetResult().ok()?;
        let pwstr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let picked = take_pwstr(pwstr);
        if picked.is_empty() {
            None
        } else {
            Some(picked)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

#[cfg(windows)]
fn ask_save_path(hwnd: isize, suggested: &Path) -> Option<PathBuf> {
    unsafe {
        use webview2_com::take_pwstr;
        use windows::core::{w, HSTRING};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{CoCreateInstance, IBindCtx, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{
            FileSaveDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM,
            FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
        };

        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().unwrap_or_default();
        dialog
            .SetOptions(opts | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT | FOS_PATHMUSTEXIST)
            .ok()?;
        let _ = dialog.SetTitle(w!("Куда сохранить файл?"));
        if let Some(name) = suggested.file_name().and_then(|n| n.to_str()) {
            let _ = dialog.SetFileName(&HSTRING::from(name));
        }
        if let Some(dir) = suggested.parent() {
            let folder: windows::core::Result<IShellItem> =
                SHCreateItemFromParsingName(&HSTRING::from(dir.to_string_lossy().as_ref()), None::<&IBindCtx>);
            if let Ok(item) = folder {
                let _ = dialog.SetFolder(&item);
            }
        }
        if dialog.Show(HWND(hwnd)).is_err() {
            return None;
        }
        let item = dialog.GetResult().ok()?;
        let pwstr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let picked = take_pwstr(pwstr);
        if picked.is_empty() {
            None
        } else {
            Some(PathBuf::from(picked))
        }
    }
}

pub(crate) fn choose_download_path(proposed: &Path, custom_dir: &str, ask: bool, hwnd: isize) -> Option<PathBuf> {
    let auto = unique_download_path(proposed, custom_dir);
    if !ask {
        return Some(auto);
    }
    #[cfg(windows)]
    {
        ask_save_path(hwnd, &auto)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        Some(auto)
    }
}

pub(crate) fn unique_download_path(proposed: &Path, custom_dir: &str) -> PathBuf {
    let dir = if !custom_dir.trim().is_empty() {
        PathBuf::from(custom_dir.trim())
    } else {
        proposed
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_download_dir)
    };
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

pub(crate) fn read_downloads(path: &Path) -> Vec<DlItem> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(4, FIELD_SEP);
            let ts = it.next()?.parse::<u64>().ok()?;
            let file_path = it.next()?.trim();
            if file_path.is_empty() || file_path.len() > MAX_URL_LEN || file_path.chars().any(|c| c.is_control()) {
                return None;
            }
            let title = clean_title(it.next().unwrap_or(""));
            let url = it.next().unwrap_or("").trim().to_string();
            Some(DlItem {
                ts,
                path: file_path.to_string(),
                title,
                url,
            })
        })
        .collect()
}

pub(crate) fn write_downloads(path: &Path, list: &[DlItem]) {
    let mut s = String::new();
    for e in list {
        s.push_str(&format!(
            "{}{}{}{}{}{}{}\n",
            e.ts, FIELD_SEP, e.path, FIELD_SEP, e.title, FIELD_SEP, e.url
        ));
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &s).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

pub(crate) fn dl_add(path: &Path, file_path: &str, title: &str, source_url: &str) {
    let mut list = read_downloads(path);
    list.retain(|e| e.path != file_path);
    if list.len() >= DOWNLOADS_MAX {
        list.remove(0);
    }
    let clean_u = if is_http_url(source_url) { source_url } else { "" };
    list.push(DlItem {
        ts: now_secs(),
        path: file_path.to_string(),
        title: clean_title(title),
        url: clean_u.to_string(),
    });
    write_downloads(path, &list);
}

pub(crate) fn dl_del(path: &Path, file_path: &str) {
    let mut list = read_downloads(path);
    let before = list.len();
    list.retain(|e| e.path != file_path);
    if list.len() != before {
        write_downloads(path, &list);
    }
}

/// Удаляет сам скачанный файл с диска и убирает запись из истории загрузок.
pub(crate) fn delete_downloaded_file(list_path: &Path, file_path: &str) {
    let p = Path::new(file_path);
    if p.exists() && p.is_file() {
        let _ = fs::remove_file(p);
    }
    dl_del(list_path, file_path);
}

pub(crate) fn dl_view(path: &Path) -> Vec<DlItem> {
    let mut list = read_downloads(path);
    list.reverse();
    list
}

pub(crate) fn dl_panel_js(list: &[DlItem]) -> String {
    let mut o = String::from("[");
    for (i, e) in list.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!(
            "{{\"u\":{},\"t\":{},\"d\":{},\"src\":{}}}",
            json_quote(&e.path),
            json_quote(&e.title),
            e.ts,
            json_quote(&e.url)
        ));
    }
    o.push(']');
    format!("if(window.__ulPanel) window.__ulPanel('downloads','{}');", js_str(&o))
}

/// Открывает скачанный файл программой по умолчанию.
pub(crate) fn open_downloaded_file(file_path: &str) {
    let p = Path::new(file_path);
    if !p.exists() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(p).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

pub(crate) fn open_downloads_folder(custom_dir: &str) {
    let dir = resolve_download_dir(custom_dir);
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

pub(crate) fn show_in_folder(file_path: &str, custom_dir: &str) {
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
            open_downloads_folder(custom_dir);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if p.exists() {
            let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
        } else {
            open_downloads_folder(custom_dir);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = p.parent().filter(|d| d.exists()) {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        } else {
            open_downloads_folder(custom_dir);
        }
    }
}
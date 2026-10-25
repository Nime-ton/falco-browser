//! Загрузки: путь сохранения, выбор папки в Проводнике, список загрузок, открытие папки.

use std::fs;
use std::path::{Path, PathBuf};
use crate::config::*;
use crate::history::*;
use crate::util::*;

pub(crate) const DOWNLOADS_MAX: usize = 500;

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

/// Открывает системное окно выбора папки в Проводнике и возвращает выбранный путь.
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

pub(crate) fn read_downloads(path: &Path) -> Vec<Entry> {
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

pub(crate) fn dl_add(path: &Path, file_path: &str, title: &str) {
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

pub(crate) fn dl_del(path: &Path, file_path: &str) {
    let mut list = read_downloads(path);
    let before = list.len();
    list.retain(|e| e.url != file_path);
    if list.len() != before {
        write_entries(path, &list);
    }
}

pub(crate) fn dl_view(path: &Path) -> Vec<Entry> {
    let mut list = read_downloads(path);
    list.reverse();
    list
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
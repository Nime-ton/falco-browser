//! Доступ к камере и микрофону.

use std::fs;
use std::path::Path;
use crate::util::*;

pub(crate) const TRUSTED_MEDIA_EXAMPLE: &str = "# Сайты, которым разрешён доступ к камере и микрофону БЕЗ вопроса.\n# По одному домену в строке, поддомены включены. Например:\n# meet.google.com\n# Остальные сайты получат обычное окно с вопросом.\n";

pub(crate) fn load_trusted_media(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && valid_domain(l))
        .collect()
}

#[cfg(windows)]
pub(crate) mod permissions {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;
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

    fn allowed(uri: &str, trusted: &[String], mode: u8) -> bool {
        if !secure_origin(uri) {
            return false;
        }
        match mode {
            2 => true,
            1 => match crate::util::host_of(uri) {
                Some(h) => h == "localhost" || h == "127.0.0.1" || trusted.iter().any(|t| crate::util::is_under(&h, t)),
                None => false,
            },
            _ => false,
        }
    }

    pub fn install(webview: &wry::WebView, trusted: Vec<String>, mode: Arc<AtomicU8>) -> windows::core::Result<()> {
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
                    if allowed(&url, &trusted, mode.load(Ordering::Relaxed)) {
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
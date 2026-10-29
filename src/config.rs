//! Общие константы, лимиты и пользовательские настройки браузера (settings.txt).

use std::fs;
use std::path::Path;
use crate::util::{is_http_url, json_quote};

pub(crate) const MAX_TABS: usize = 50;

pub(crate) const MAX_CLOSED_TABS: usize = 25;

pub(crate) const MAX_URL_LEN: usize = 4096;

pub(crate) const MAX_IPC_LEN: usize = 8192;

pub(crate) const MAX_IPC_PER_SEC: u32 = 60;

pub(crate) const PERF_LOG: bool = false;

pub(crate) const DEFAULT_URL: &str = "https://duckduckgo.com";

#[derive(Clone)]
pub(crate) struct Settings {
    pub(crate) start_url: String,
    /// ddg | google | yandex | bing | brave
    pub(crate) search_engine: String,
    pub(crate) restore_session: bool,
    pub(crate) download_dir: String,
    pub(crate) history_enabled: bool,
    /// 0 = спрашивать всегда, 1 = без вопроса для trusted-media.txt, 2 = разрешать всем https
    pub(crate) media_mode: u8,
    pub(crate) smartscreen: bool,
    pub(crate) webrtc_hide_ip: bool,
    pub(crate) yt_force_h264: bool,
    /// Закрепить верхнюю панель (вкладки и адресную строку), чтобы она не пряталась и не перекрывала сайты
    pub(crate) pin_top_bar: bool,
    /// Закреплённые вкладки "живые": у каждой свой WebView, который продолжает работать в фоне
    /// (музыка, видео, звонки), пока вы сидите в обычных вкладках
    pub(crate) live_pinned: bool,
    /// Перед каждой загрузкой спрашивать, куда сохранить файл (окно "Сохранить как")
    pub(crate) ask_download: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_url: DEFAULT_URL.to_string(),
            search_engine: "ddg".to_string(),
            restore_session: true,
            download_dir: String::new(),
            history_enabled: true,
            media_mode: 1,
            smartscreen: true,
            webrtc_hide_ip: true,
            yt_force_h264: false,
            pin_top_bar: false,
            live_pinned: true,
            ask_download: false,
        }
    }
}

impl Settings {
    pub(crate) fn load(path: &Path) -> Self {
        let mut s = Self::default();
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                s.save(path);
                return s;
            }
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                match k {
                    "start_url" if is_http_url(v) => s.start_url = v.to_string(),
                    "search_engine" if matches!(v, "ddg" | "google" | "yandex" | "bing" | "brave") => {
                        s.search_engine = v.to_string();
                    }
                    "restore_session" => s.restore_session = v == "1" || v.eq_ignore_ascii_case("true"),
                    "download_dir" if v.len() <= 500 && !v.chars().any(|c| c.is_control()) => {
                        s.download_dir = v.to_string();
                    }
                    "history_enabled" => s.history_enabled = v == "1" || v.eq_ignore_ascii_case("true"),
                    "media_mode" => {
                        if let Ok(m) = v.parse::<u8>() {
                            s.media_mode = m.min(2);
                        }
                    }
                    "smartscreen" => s.smartscreen = v == "1" || v.eq_ignore_ascii_case("true"),
                    "webrtc_hide_ip" => s.webrtc_hide_ip = v == "1" || v.eq_ignore_ascii_case("true"),
                    "yt_force_h264" => s.yt_force_h264 = v == "1" || v.eq_ignore_ascii_case("true"),
                    "pin_top_bar" => s.pin_top_bar = v == "1" || v.eq_ignore_ascii_case("true"),
                    "live_pinned" => s.live_pinned = v == "1" || v.eq_ignore_ascii_case("true"),
                    "ask_download" => s.ask_download = v == "1" || v.eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }
        s
    }

    pub(crate) fn save(&self, path: &Path) {
        let content = format!(
            "# Настройки браузера Falco\n\
             start_url={}\n\
             search_engine={}\n\
             restore_session={}\n\
             download_dir={}\n\
             history_enabled={}\n\
             media_mode={}\n\
             smartscreen={}\n\
             webrtc_hide_ip={}\n\
             yt_force_h264={}\n\
             pin_top_bar={}\n\
             live_pinned={}\n\
             ask_download={}\n",
            self.start_url,
            self.search_engine,
            if self.restore_session { 1 } else { 0 },
            self.download_dir,
            if self.history_enabled { 1 } else { 0 },
            self.media_mode,
            if self.smartscreen { 1 } else { 0 },
            if self.webrtc_hide_ip { 1 } else { 0 },
            if self.yt_force_h264 { 1 } else { 0 },
            if self.pin_top_bar { 1 } else { 0 },
            if self.live_pinned { 1 } else { 0 },
            if self.ask_download { 1 } else { 0 },
        );
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, content).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }

    pub(crate) fn search_url_prefix(&self) -> &'static str {
        match self.search_engine.as_str() {
            "google" => "https://www.google.com/search?q=",
            "yandex" => "https://ya.ru/search/?text=",
            "bing" => "https://www.bing.com/search?q=",
            "brave" => "https://search.brave.com/search?q=",
            _ => "https://duckduckgo.com/?q=",
        }
    }

    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\"start_url\":{},\"search_engine\":{},\"search_prefix\":{},\"restore_session\":{},\"download_dir\":{},\"history_enabled\":{},\"media_mode\":{},\"smartscreen\":{},\"webrtc_hide_ip\":{},\"yt_force_h264\":{},\"pin_top_bar\":{},\"live_pinned\":{},\"ask_download\":{}}}",
            json_quote(&self.start_url),
            json_quote(&self.search_engine),
            json_quote(self.search_url_prefix()),
            self.restore_session,
            json_quote(&self.download_dir),
            self.history_enabled,
            self.media_mode,
            self.smartscreen,
            self.webrtc_hide_ip,
            self.yt_force_h264,
            self.pin_top_bar,
            self.live_pinned,
            self.ask_download
        )
    }
}
//! Пользовательские скрипты (*.js в папке scripts, формат UserScript).

use std::fs;
use std::path::Path;
use crate::util::*;

pub(crate) const USER_SCRIPT_TEMPLATE: &str = include_str!("../js/userscript.js");

pub(crate) const EXAMPLE_SCRIPT: &str = r##"// ==UserScript==
// @name        Пример (файлы, имя которых начинается с _, игнорируются - уберите подчёркивание)
// @match       http://127.0.0.1:7860/*
// @run-at      document-idle
// ==/UserScript==
GM_addStyle('footer { display: none !important; }');
"##;

pub(crate) fn load_user_scripts(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for path in list_files(dir, "js") {
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let code = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string();
        let mut name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("script").to_string();
        let mut run_at: &str = "document-idle";
        let mut noframes = false;
        let (mut m, mut i, mut e, mut ex): (Vec<String>, Vec<String>, Vec<String>, Vec<String>) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut in_header = false;
        for line in code.lines() {
            let l = line.trim();
            if l.starts_with("// ==UserScript==") {
                in_header = true;
                continue;
            }
            if l.starts_with("// ==/UserScript==") {
                break;
            }
            if !in_header {
                continue;
            }
            let l = match l.strip_prefix("//") {
                Some(r) => r.trim_start(),
                None => continue,
            };
            let l = match l.strip_prefix('@') {
                Some(r) => r,
                None => continue,
            };
            let (key, val) = match l.split_once(char::is_whitespace) {
                Some((k, v)) => (k, v.trim()),
                None => (l, ""),
            };
            match key {
                "name" if !val.is_empty() => name = val.to_string(),
                "match" => m.push(val.to_string()),
                "include" => i.push(val.to_string()),
                "exclude" => e.push(val.to_string()),
                "exclude-match" => ex.push(val.to_string()),
                "run-at" => {
                    run_at = match val {
                        "document-start" => "document-start",
                        "document-end" | "document-body" => "document-end",
                        _ => "document-idle",
                    }
                }
                "noframes" => noframes = true,
                _ => {}
            }
        }
        if m.is_empty() && i.is_empty() {
            continue;
        }
        let js = USER_SCRIPT_TEMPLATE
            .replace("__M__", &json_arr(&m))
            .replace("__I__", &json_arr(&i))
            .replace("__EX__", &json_arr(&ex))
            .replace("__E__", &json_arr(&e))
            .replace("__NAMEJSON__", &json_str(&name))
            .replace("__RT__", run_at)
            .replace("__NF__", if noframes { "true" } else { "false" })
            .replace("__CODE__", &code);
        out.push(js);
    }
    out
}
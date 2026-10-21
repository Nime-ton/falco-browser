fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    #[cfg(windows)]
    {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let icon = std::path::Path::new(&dir).join("icon.ico");
        if icon.exists() {
            let mut res = winres::WindowsResource::new();
            res.set_icon(&icon.to_string_lossy().replace('\\', "/"));
            res.compile().unwrap();
        } else {
            println!("cargo:warning=Не найден файл иконки: {}", icon.display());
        }
    }
}
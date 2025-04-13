fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/chart-line-solid.ico");
        #[cfg(target_env = "gnu")]
        res.set_toolkit_path("/usr/bin")
            .set_windres_path("x86_64-w64-mingw32-windres");
        res.compile().expect("Failed to compile Windows resource");
    }
}

#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::time::Duration;
use tokio::runtime::Runtime;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    // tokio threading to not block ui on long threads, see:
    // https://github.com/emilk/egui/discussions/521#discussioncomment-3462382
    // https://github.com/parasyte/egui-tokio-example/blob/main/src/main.rs
    let rt = Runtime::new().expect("Unable to create Runtime");

    // Enter the runtime so that `tokio::spawn` is available immediately.
    let _enter = rt.enter();

    // Execute the runtime in its own thread.
    // The future doesn't have to do anything. In this example, it just sleeps forever.
    std::thread::spawn(move || {
        rt.block_on(async {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        })
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 1000.0])
            .with_min_inner_size([300.0, 220.0])
            .with_title("RustyMeter")
            .with_icon(load_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "RustyMeter",
        native_options,
        Box::new(|cc| {
            // This gives us image support:
            egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(Box::new(rusty_meter::MyApp::new(cc)))
        }),
    )
}

// When compiling to web using trunk:
#[cfg(target_arch = "wasm32")]
fn main() {
    // Redirect `log` message to `console.log` and friends:
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        eframe::WebRunner::new()
            .start(
                "the_canvas_id", // hardcode it
                web_options,
                Box::new(|cc| Box::new(rusty_meter::MyApp::new(cc))),
            )
            .await
            .expect("failed to start eframe");
    });
}

// Function to load the icon (supports both Windows and others)
fn load_icon() -> egui::viewport::IconData {
    #[cfg(target_os = "windows")]
    {
        // Load ICO for Windows
        let image = image::open("assets/chart-line-solid.ico")
            .expect("Failed to open icon")
            .to_rgba8();
        let (width, height) = image.dimensions();
        egui::viewport::IconData {
            rgba: image.into_raw(),
            width,
            height,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Keep PNG for macOS/Linux
        eframe::icon_data::from_png_bytes(&include_bytes!("../assets/chart-line-solid.png")[..])
            .expect("Failed to load icon")
    }
}

#![warn(clippy::all, rust_2018_idioms)]
#![allow(clippy::collapsible_if)]

mod app;
pub use app::MyApp;
mod helpers;
mod multimeter;
#[cfg(not(target_arch = "wasm32"))]
mod victor;

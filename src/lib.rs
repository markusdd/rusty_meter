#![warn(clippy::all, rust_2018_idioms)]
#![allow(clippy::collapsible_if)]

mod app;
pub use app::MyApp;
mod helpers;
mod multimeter;
#[cfg(not(target_arch = "wasm32"))]
pub mod victor_86bcd_capture;
#[cfg(not(target_arch = "wasm32"))]
pub mod victor_dm1107;
#[cfg(not(target_arch = "wasm32"))]
mod victor_es519xx;
#[cfg(not(target_arch = "wasm32"))]
mod victor_fs9922;

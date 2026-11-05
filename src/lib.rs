#![warn(clippy::all, rust_2018_idioms)]
#![allow(clippy::collapsible_if)]

mod app;
pub use app::MyApp;
pub mod agent_proto;
mod helpers;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp_stdio;
mod multimeter;
#[cfg(not(target_arch = "wasm32"))]
pub mod profile;
mod psu;
mod scpi_macro;
#[cfg(not(target_arch = "wasm32"))]
pub mod victor_86bcd_capture;
#[cfg(not(target_arch = "wasm32"))]
pub mod victor_dm1107;
#[cfg(not(target_arch = "wasm32"))]
mod victor_es519xx;
#[cfg(not(target_arch = "wasm32"))]
mod victor_fs9922;

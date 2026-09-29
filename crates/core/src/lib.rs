//! Núcleo de LiquidControl: acceso nativo (Win32, COM y WinRT) a la configuración de Windows.
//!
//! Cada módulo expone funciones **síncronas**: la interfaz las llama desde
//! hilos de trabajo para no bloquear la ventana. Nada de lo que recibe la API
//! son rutas de registro ni comandos: solo identificadores y valores tipados que
//! se validan acá.

#[cfg(not(windows))]
compile_error!("LiquidControl solo funciona en Windows.");

pub mod accessibility;
pub mod actions;
pub mod activity;
pub mod airdrop;
pub mod appearance;
pub mod apps;
pub mod bluetooth;
pub mod displays;
pub mod error;
pub mod icloud;
pub mod icons;
pub mod input;
pub mod keyboard;
pub mod leftovers;
pub mod media;
pub mod mixer;
pub mod net;
pub mod network;
pub mod notifications;
pub mod openverse;
pub mod paths;
pub mod picker;
pub mod power;
pub mod security;
pub mod sound;
pub mod system;
pub mod tweaks;
pub mod util;
pub mod wallhaven;
pub mod wallpaper;
pub mod wallpaper_search;
pub mod wifi;
pub mod window;

#[cfg(test)]
mod smoke;

pub use error::{AppError, AppResult};

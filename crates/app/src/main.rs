//! LiquidControl: panel de configuración para Windows con la estética de Ajustes
//! del Sistema de macOS. Interfaz nativa con Slint; toda la lógica del sistema
//! vive en `liquidcontrol-core`.

// Sin consola en las compilaciones de lanzamiento: la actividad se ve en la
// consola integrada (Ajustes de la app → Consola de actividad).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Código generado a partir de `ui/*.slint`.
#[allow(clippy::all, unused_qualifications)]
mod ui {
    slint::include_modules!();
}
use ui::*;

mod account;
mod bridge;
mod chrome;
mod commands;
mod console;
mod dialogs;
mod format;
mod glyphs;
mod images;
mod models;
mod nav;
mod panes;
mod photo_search;
mod prefs;
mod search;
mod toasts;

fn main() -> Result<(), slint::PlatformError> {
    // Primero el registro de actividad: así queda anotado todo lo que sigue.
    liquidcontrol_core::activity::init();
    log::info!(
        target: "app",
        "LiquidControl {} iniciado{}",
        env!("CARGO_PKG_VERSION"),
        if liquidcontrol_core::actions::is_admin() { " como administrador" } else { "" }
    );
    chrome::select_backend()?;
    let app = AppWindow::new()?;
    prefs::init(&app);
    chrome::init(&app);
    toasts::init(&app);
    dialogs::init(&app);
    console::init(&app);
    commands::init(&app);
    search::init(&app);
    account::load(&app);
    panes::init(&app);
    let (pane, sub) = start_location();
    nav::init(&app, &pane, &sub);
    app.run()
}

/// `--pane <id> [--sub <id>]` abre la app directamente en un panel (lo usa el
/// relanzamiento como administrador). Los ids desconocidos se ignoran.
fn start_location() -> (String, String) {
    let (mut pane, mut sub) = (String::new(), String::new());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pane" => pane = args.next().unwrap_or_default(),
            "--sub" => sub = args.next().unwrap_or_default(),
            _ => {}
        }
    }
    (pane, sub)
}

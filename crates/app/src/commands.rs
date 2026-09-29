//! Acciones generales que pide la interfaz: abrir enlaces del sistema, reiniciar
//! el Explorador y relanzar la app como administrador.

use liquidcontrol_core::actions;
use slint::ComponentHandle;

use crate::toasts::{self, Kind};
use crate::{App, AppWindow, bridge, nav, panes};

pub fn init(app: &AppWindow) {
    let global = app.global::<App>();
    global.set_admin(actions::is_admin());
    global.set_version(env!("CARGO_PKG_VERSION").into());

    let weak = app.as_weak();
    global.on_open_uri(move |uri| {
        let Some(app) = weak.upgrade() else { return };
        let uri = uri.to_string();
        bridge::spawn(
            &app,
            move || actions::open_uri(&uri),
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    global.on_relaunch_admin(move || {
        if let Some(app) = weak.upgrade() {
            relaunch_admin(&app);
        }
    });

    let weak = app.as_weak();
    global.on_restart_explorer(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(&app, actions::restart_explorer, |app, result| match result {
            Ok(()) => {
                panes::explorer_restarted(app);
                toasts::show(Kind::Success, "Se reinició el Explorador.");
            }
            Err(err) => toasts::error(&err),
        });
    });
}

/// Relanza la app con UAC en el panel actual y, si el usuario acepta, cierra esta instancia.
pub fn relaunch_admin(app: &AppWindow) {
    let args = format!("--pane {}", nav::current_pane(app));
    bridge::spawn(
        app,
        move || actions::relaunch_as_admin(&args),
        |_, result| match result {
            Ok(()) => {
                let _ = slint::quit_event_loop();
            }
            // El usuario dijo que no en el UAC: no hace falta avisar nada.
            Err(err) if err.code() == "cancelled" => {}
            Err(err) => toasts::error(&err),
        },
    );
}

//! Cuenta de Windows: perfil, tipo de cuenta e inicio de sesión (los datos los
//! carga `crate::account` al iniciar). iCloud vive como subpágina de este panel.

use super::{Pane, fixed_target, fixed_title, icloud};
use crate::AppWindow;

pub const PANE: Pane = Pane::new("account", enter).with_leave(leave).with_subpages(sub_title, target);

const SUBPAGES: [(&str, &str); 2] = [("icloud", "iCloud"), ("icloud-photos", "Fotos de iCloud")];

fn enter(app: &AppWindow, sub: &str) {
    match sub {
        "icloud" => icloud::enter(app),
        "icloud-photos" => icloud::enter_all(app),
        _ => {}
    }
}

fn leave(app: &AppWindow, sub: &str) {
    match sub {
        "icloud" => icloud::leave(app),
        "icloud-photos" => icloud::leave_all(app),
        _ => {}
    }
}

fn sub_title(sub: &str) -> Option<String> {
    fixed_title(&SUBPAGES, sub)
}

fn target(item: &str) -> (String, String) {
    fixed_target(&SUBPAGES, item)
}

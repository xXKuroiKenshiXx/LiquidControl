//! Accesibilidad: lupa, movimiento y transparencia (banderas compartidas con
//! Apariencia) y los ajustes del catálogo de esta categoría.

use liquidcontrol_core::accessibility;
use slint::ComponentHandle;

use super::tweaks::{self, Category};
use super::{Pane, appearance};
use crate::{AccessibilityState, AppWindow, bridge, toasts};

pub const PANE: Pane = Pane::new("accessibility", |app, _| enter(app)).with_init(init);

fn init(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<AccessibilityState>().on_set_magnifier(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<AccessibilityState>().set_magnifier(on);
        bridge::spawn(
            &app,
            move || accessibility::set_magnifier(on),
            move |app, result| {
                if let Err(err) = result {
                    app.global::<AccessibilityState>().set_magnifier(!on);
                    toasts::error(&err);
                }
            },
        );
    });
}

fn enter(app: &AppWindow) {
    appearance::load(app);
    tweaks::load(app, Category::Accessibility);
    bridge::spawn(app, accessibility::magnifier_on, |app, on| app.global::<AccessibilityState>().set_magnifier(on));
}

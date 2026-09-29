//! Optimización: punto de restauración, ajustes de rendimiento y apps preinstaladas.

use liquidcontrol_core::apps::{self, AppxPackage};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::Pane;
use super::tweaks::{self, Category};
use crate::toasts::{self, Kind};
use crate::{AppWindow, AppxView, OptimizeState, bridge, dialogs, format};

pub const PANE: Pane = Pane::new("optimize", |app, _| enter(app)).with_init(init);

const REMOVE_DIALOG: &str = "appx-remove";

pub fn init(app: &AppWindow) {
    let state = app.global::<OptimizeState>();

    let weak = app.as_weak();
    state.on_create_restore_point(move || {
        let Some(app) = weak.upgrade() else { return };
        app.global::<OptimizeState>().set_restore_busy(true);
        bridge::spawn(&app, apps::create_restore_point, |app, result| {
            app.global::<OptimizeState>().set_restore_busy(false);
            match result {
                Ok(()) => toasts::show(Kind::Success, "Punto de restauración creado."),
                Err(err) => toasts::error(&err),
            }
        });
    });

    let weak = app.as_weak();
    state.on_toggle_appx(move |name, selected| {
        let Some(app) = weak.upgrade() else { return };
        let model = app.global::<OptimizeState>().get_appx();
        if let Some((index, mut view)) = model.iter().enumerate().find(|(_, view)| view.name == name) {
            view.selected = selected;
            model.set_row_data(index, view);
        }
        update_count(&app);
    });

    let weak = app.as_weak();
    state.on_remove_appx(move || {
        let Some(app) = weak.upgrade() else { return };
        let labels: Vec<String> = chosen(&app).iter().map(|view| view.label.to_string()).collect();
        if labels.is_empty() {
            return;
        }
        let message = format!(
            "{}.\n\nSe eliminan para tu cuenta de usuario. Podés reinstalarlas desde la Microsoft Store.",
            labels.join(", ")
        );
        dialogs::confirm(&app, REMOVE_DIALOG, &format!("¿Eliminar {} apps?", labels.len()), &message, "Eliminar", true);
    });

    dialogs::register(REMOVE_DIALOG, |app, _| {
        let names: Vec<String> = chosen(app).iter().map(|view| view.name.to_string()).collect();
        let total = names.len();
        dialogs::set_busy(app, true);
        bridge::spawn(
            app,
            move || apps::appx_remove(&names),
            move |app, result| {
                dialogs::close(app);
                match result {
                    Ok(removed) => {
                        toasts::show(Kind::Success, format!("Se eliminaron {} de {total} apps.", removed.len()));
                        load_appx(app);
                    }
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });
}

pub fn enter(app: &AppWindow) {
    tweaks::load(app, Category::Optimize);
    load_appx(app);
}

fn load_appx(app: &AppWindow) {
    bridge::spawn(app, apps::appx_list, |app, result| {
        let state = app.global::<OptimizeState>();
        match result {
            Ok(list) => {
                let views: Vec<AppxView> = list.iter().filter(|package| package.installed).map(view).collect();
                state.set_appx(ModelRc::new(VecModel::from(views)));
            }
            Err(err) => toasts::error(&err),
        }
        state.set_appx_loaded(true);
        update_count(app);
    });
}

fn view(package: &AppxPackage) -> AppxView {
    AppxView {
        name: package.name.as_str().into(),
        label: package.label.as_str().into(),
        tint: format::tint_index(&package.name),
        selected: false,
    }
}

fn chosen(app: &AppWindow) -> Vec<AppxView> {
    app.global::<OptimizeState>().get_appx().iter().filter(|view| view.selected).collect()
}

fn update_count(app: &AppWindow) {
    let count = chosen(app).len();
    app.global::<OptimizeState>().set_appx_selected(count as i32);
}

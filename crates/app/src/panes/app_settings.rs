//! Ajustes de la app: tema, acento, tamaño de la interfaz, animaciones, Liquid
//! Glass, botones de ventana, íconos del catálogo, datos y la consola de
//! actividad (subpágina).

use liquidcontrol_core::{actions, paths};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Pane, fixed_target, fixed_title};
use crate::prefs::{self, MOTIONS, Prefs, ThemeMode, WindowControls, WindowCorners, ZOOMS};
use crate::{AppSettingsState, AppWindow, Theme, bridge, chrome, toasts};

pub const PANE: Pane = Pane::new("app", |app, _| show(app)).with_init(init).with_subpages(sub_title, target);

const SUBPAGES: [(&str, &str); 1] = [("console", "Consola de actividad")];

/// Nombres para la interfaz: primero el acento de Windows y después los de `prefs::ACCENTS`.
const ACCENT_LABELS: [&str; 10] = [
    "Acento de Windows",
    "Azul",
    "Morado",
    "Rosa",
    "Rojo",
    "Naranja",
    "Amarillo",
    "Verde",
    "Grafito",
    "Vidrio líquido",
];

fn init(app: &AppWindow) {
    let state = app.global::<AppSettingsState>();
    let labels: Vec<SharedString> = ACCENT_LABELS.iter().map(|label| (*label).into()).collect();
    state.set_accent_labels(ModelRc::new(VecModel::from(labels)));
    // "Vidrio líquido" siempre es la última muestra (ver `prefs::ACCENTS`).
    state.set_accent_glass_index(prefs::ACCENTS.len() as i32);

    let weak = app.as_weak();
    state.on_set_theme(move |index| {
        if let Some(app) = weak.upgrade() {
            save(&app, |prefs| {
                prefs.theme = match index {
                    1 => ThemeMode::Light,
                    2 => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
            });
        }
    });

    let weak = app.as_weak();
    state.on_set_accent(move |index| {
        if let Some(app) = weak.upgrade() {
            let name = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| prefs::ACCENTS.get(i))
                .map_or("system", |(name, ..)| name);
            save(&app, |prefs| prefs.accent = name.to_owned());
        }
    });

    let weak = app.as_weak();
    state.on_set_controls(move |index| {
        if let Some(app) = weak.upgrade() {
            save(&app, |prefs| {
                prefs.window_controls = if index == 1 { WindowControls::Windows } else { WindowControls::Mac };
            });
        }
    });

    let weak = app.as_weak();
    state.on_set_corners(move |index| {
        if let Some(app) = weak.upgrade() {
            save(&app, |prefs| {
                prefs.window_corners = if index == 1 { WindowCorners::Mac } else { WindowCorners::Native };
            });
        }
    });

    let weak = app.as_weak();
    state.on_set_zoom(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&zoom) = usize::try_from(index).ok().and_then(|i| ZOOMS.get(i)) else { return };
        save(&app, |prefs| prefs.zoom = zoom);
        // Un tamaño de interfaz más grande pide más píxeles físicos para el
        // mismo contenido: si ya no entra en la pantalla, se vuelve a ajustar.
        chrome::fit_to_screen(&app);
    });

    let weak = app.as_weak();
    state.on_set_motion(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&motion) = usize::try_from(index).ok().and_then(|i| MOTIONS.get(i)) else { return };
        save(&app, |prefs| prefs.motion = motion);
    });

    // El vidrio se ve en vivo mientras se arrastra; se guarda al soltar.
    let weak = app.as_weak();
    state.on_glass_moved(move |percent| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<AppSettingsState>().set_glass(percent);
        app.global::<Theme>().set_glass((percent / 100.0).clamp(0.0, prefs::MAX_GLASS));
    });

    let weak = app.as_weak();
    state.on_glass_released(move |percent| {
        if let Some(app) = weak.upgrade() {
            save(&app, |prefs| prefs.glass = percent / 100.0);
        }
    });

    let weak = app.as_weak();
    state.on_set_catalog_icons(move |on| {
        if let Some(app) = weak.upgrade() {
            save(&app, |prefs| prefs.catalog_icons = on);
        }
    });

    let weak = app.as_weak();
    state.on_open_data_folder(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(
            &app,
            || {
                let dir = paths::data_dir()?;
                std::fs::create_dir_all(&dir)?;
                actions::open_folder(&dir)
            },
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });
}

fn sub_title(sub: &str) -> Option<String> {
    fixed_title(&SUBPAGES, sub)
}

fn target(item: &str) -> (String, String) {
    fixed_target(&SUBPAGES, item)
}

fn save(app: &AppWindow, change: impl FnOnce(&mut Prefs)) {
    if let Err(err) = prefs::update(app, change) {
        toasts::error(&err);
    }
    show(app);
}

fn show(app: &AppWindow) {
    let current = prefs::get();
    let state = app.global::<AppSettingsState>();
    let dark = app.global::<Theme>().get_dark();
    state.set_theme(match current.theme {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    });
    let accent = prefs::ACCENTS.iter().position(|(name, ..)| *name == current.accent).map_or(0, |i| i as i32 + 1);
    state.set_accent(accent);
    let colors: Vec<Color> = std::iter::once("system")
        .chain(prefs::ACCENTS.iter().map(|(name, ..)| *name))
        .map(|name| prefs::accent_color(name, dark))
        .collect();
    state.set_accent_colors(ModelRc::new(VecModel::from(colors)));
    state.set_controls(i32::from(current.window_controls == WindowControls::Windows));
    state.set_corners(i32::from(current.window_corners == WindowCorners::Mac));
    state.set_zoom(nearest(&ZOOMS, current.zoom));
    state.set_motion(nearest(&MOTIONS, current.motion));
    state.set_glass((current.glass * 100.0).round());
    state.set_catalog_icons(current.catalog_icons);
    state.set_portable(paths::is_portable());
    state.set_data_folder(paths::data_dir().map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default().into());
}

/// Opción más cercana a un valor guardado (el archivo puede tener cualquier número).
fn nearest(options: &[f32], value: f32) -> i32 {
    options
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - value).abs().total_cmp(&(*b - value).abs()))
        .map_or(0, |(index, _)| index as i32)
}

#[cfg(test)]
mod tests {
    use super::nearest;
    use crate::prefs::{MOTIONS, ZOOMS};

    #[test]
    fn picks_the_nearest_option() {
        assert_eq!(nearest(&ZOOMS, 1.0), 1);
        assert_eq!(nearest(&ZOOMS, 1.2), 2);
        assert_eq!(nearest(&MOTIONS, 0.0), 0);
        assert_eq!(nearest(&MOTIONS, 5.0), 3);
    }
}

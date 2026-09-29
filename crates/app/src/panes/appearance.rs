//! Apariencia de Windows: modo claro/oscuro/mixto, color de acento (se aplica
//! directamente, sin pasar por Configuración) y efectos visuales.
//! Accesibilidad reutiliza sus banderas. El fondo de pantalla es otro panel.

use liquidcontrol_core::AppResult;
use liquidcontrol_core::appearance::{self, AppearanceFlag, Mode, Rgb};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use super::Pane;
use crate::{AppWindow, AppearanceState, bridge, prefs, toasts};

pub const PANE: Pane = Pane::new("appearance", |app, _| load(app)).with_init(init);

/// Colores de acento de Windows 11 que se ofrecen (los mismos que en Configuración).
const ACCENTS: [(Rgb, &str); 10] = [
    ((0x00, 0x78, 0xd4), "Azul"),
    ((0x74, 0x4d, 0xa9), "Iris"),
    ((0x88, 0x17, 0x98), "Violeta"),
    ((0xe3, 0x00, 0x8c), "Ciruela"),
    ((0xe8, 0x11, 0x23), "Rojo"),
    ((0xf7, 0x63, 0x0c), "Naranja"),
    ((0xff, 0xb9, 0x00), "Dorado"),
    ((0x10, 0x89, 0x3e), "Verde"),
    ((0x00, 0x99, 0xbc), "Azul frío"),
    ((0x7a, 0x75, 0x74), "Gris"),
];
/// Diferencia máxima (suma por canal) para reconocer un acento de la lista.
const MATCH_DISTANCE: u32 = 24;

fn init(app: &AppWindow) {
    let state = app.global::<AppearanceState>();
    let colors: Vec<Color> = ACCENTS.iter().map(|((r, g, b), _)| Color::from_rgb_u8(*r, *g, *b)).collect();
    state.set_accent_colors(ModelRc::new(VecModel::from(colors)));
    let labels: Vec<SharedString> = ACCENTS.iter().map(|(_, name)| (*name).into()).collect();
    state.set_accent_labels(ModelRc::new(VecModel::from(labels)));

    let weak = app.as_weak();
    state.on_set_mode(move |mode| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<AppearanceState>();
        if state.get_mode() == mode {
            return;
        }
        // Mixto: apps claras con Inicio y barra de tareas oscuros.
        let (apps, system) = match mode {
            0 => (Mode::Light, Mode::Light),
            1 => (Mode::Dark, Mode::Dark),
            _ => (Mode::Light, Mode::Dark),
        };
        state.set_mode(mode);
        state.set_system_light(system == Mode::Light);
        bridge::spawn(
            &app,
            move || appearance::set_theme(apps, system),
            |app, result| match result {
                // Si LiquidControl sigue el tema del sistema, cambia junto con Windows.
                Ok(()) => prefs::apply(app),
                Err(err) => {
                    load(app);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_flag(move |flag, value| {
        let Some(app) = weak.upgrade() else { return };
        let Some(flag) = parse_flag(&flag) else { return };
        set_local(&app, flag, value);
        bridge::spawn(
            &app,
            move || appearance::set_flag(flag, value),
            |app, result| {
                if let Err(err) = result {
                    load(app);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_accent(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&((r, g, b), _)) = usize::try_from(index).ok().and_then(|i| ACCENTS.get(i)) else { return };
        let state = app.global::<AppearanceState>();
        state.set_accent_index(index);
        state.set_auto_accent(false);
        state.set_accent(Color::from_rgb_u8(r, g, b));
        bridge::spawn(&app, move || appearance::set_accent((r, g, b)), after_accent_change);
    });

    let weak = app.as_weak();
    state.on_set_auto_accent(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<AppearanceState>().set_auto_accent(on);
        bridge::spawn(&app, move || appearance::set_auto_accent(on), after_accent_change);
    });
}

fn after_accent_change(app: &AppWindow, result: AppResult<()>) {
    match result {
        // Si LiquidControl usa el acento de Windows, cambia junto con él.
        Ok(()) => prefs::apply(app),
        Err(err) => toasts::error(&err),
    }
    load(app);
}

pub fn load(app: &AppWindow) {
    bridge::spawn(app, appearance::get, |app, look| {
        let state = app.global::<AppearanceState>();
        state.set_mode(match (look.apps_light, look.system_light) {
            (true, true) => 0,
            (false, false) => 1,
            _ => 2,
        });
        state.set_system_light(look.system_light);
        state.set_accent_on_taskbar(look.accent_on_taskbar);
        state.set_accent_on_title_bars(look.accent_on_title_bars);
        state.set_transparency(look.transparency);
        state.set_animations(look.animations);
        state.set_scrollbars(look.always_show_scrollbars);
        state.set_auto_accent(look.auto_accent);
        let (r, g, b) = look.accent;
        state.set_accent(Color::from_rgb_u8(r, g, b));
        state.set_accent_index(accent_index(look.accent));
        state.set_loaded(true);
    });
}

/// Posición del acento actual en la lista (-1 si se eligió otro en Configuración).
fn accent_index(color: Rgb) -> i32 {
    ACCENTS.iter().position(|(rgb, _)| distance(*rgb, color) <= MATCH_DISTANCE).map_or(-1, |index| index as i32)
}

fn distance(a: Rgb, b: Rgb) -> u32 {
    u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1)) + u32::from(a.2.abs_diff(b.2))
}

fn parse_flag(name: &str) -> Option<AppearanceFlag> {
    Some(match name {
        "transparency" => AppearanceFlag::Transparency,
        "accent-taskbar" => AppearanceFlag::AccentOnTaskbar,
        "accent-title-bars" => AppearanceFlag::AccentOnTitleBars,
        "animations" => AppearanceFlag::Animations,
        "scrollbars" => AppearanceFlag::AlwaysShowScrollbars,
        _ => return None,
    })
}

fn set_local(app: &AppWindow, flag: AppearanceFlag, value: bool) {
    let state = app.global::<AppearanceState>();
    match flag {
        AppearanceFlag::Transparency => state.set_transparency(value),
        AppearanceFlag::AccentOnTaskbar => state.set_accent_on_taskbar(value),
        AppearanceFlag::AccentOnTitleBars => state.set_accent_on_title_bars(value),
        AppearanceFlag::Animations => state.set_animations(value),
        AppearanceFlag::AlwaysShowScrollbars => state.set_scrollbars(value),
    }
}

#[cfg(test)]
mod tests {
    use super::accent_index;

    #[test]
    fn recognizes_listed_accents() {
        assert_eq!(accent_index((0x00, 0x78, 0xd4)), 0);
        assert_eq!(accent_index((0x02, 0x7a, 0xd0)), 0);
        assert_eq!(accent_index((0x40, 0x40, 0x40)), -1);
    }
}

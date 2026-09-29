//! Pantallas: distribución, resolución y frecuencia (con confirmación que se
//! revierte sola si la imagen no se ve bien) y brillo por DDC/CI.

use std::cell::RefCell;
use std::time::Duration;

use liquidcontrol_core::displays::{self, DisplayInfo, DisplayMode};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::Pane;
use crate::{AppWindow, DisplaysState, ScreenView, bridge, dialogs, toasts};

pub const PANE: Pane = Pane::new("displays", |app, _| load(app)).with_init(init);

const CONFIRM_SECONDS: i32 = 15;
const CONFIRM_DIALOG: &str = "display-keep";
/// Caja máxima del dibujo de la distribución (px lógicos).
const CANVAS: (f32, f32) = (420.0, 120.0);
const BRIGHTNESS_DELAY: Duration = Duration::from_millis(80);

#[derive(Default)]
struct State {
    list: Vec<DisplayInfo>,
    selected: usize,
    /// Resoluciones únicas del monitor elegido, de mayor a menor.
    resolutions: Vec<(u32, u32)>,
    /// Frecuencias disponibles para la resolución actual, de mayor a menor.
    rates: Vec<u32>,
    pending: Option<Pending>,
}

/// Cambio de modo a la espera de que el usuario lo confirme.
struct Pending {
    id: String,
    previous: DisplayMode,
    seconds: i32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static COUNTDOWN: Timer = Timer::default();
    static BRIGHTNESS: Timer = Timer::default();
    static NIGHT_LIGHT: Timer = Timer::default();
}

fn init(app: &AppWindow) {
    let state = app.global::<DisplaysState>();

    let weak = app.as_weak();
    state.on_set_hdr(move |on| {
        let Some(app) = weak.upgrade() else { return };
        let Some(id) = STATE.with_borrow(|state| state.list.get(state.selected).map(|d| d.id.clone())) else { return };
        app.global::<DisplaysState>().set_hdr(on);
        bridge::spawn(
            &app,
            move || displays::set_hdr(&id, on),
            move |app, result| match result {
                Ok(()) => load(app),
                Err(err) => {
                    app.global::<DisplaysState>().set_hdr(!on);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_select(move |index| {
        let Some(app) = weak.upgrade() else { return };
        STATE.with_borrow_mut(|state| {
            if let Ok(index) = usize::try_from(index) {
                if index < state.list.len() {
                    state.selected = index;
                }
            }
        });
        show(&app);
    });

    let weak = app.as_weak();
    state.on_set_resolution(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let change = STATE.with_borrow(|state| {
            let display = state.list.get(state.selected)?;
            let &(width, height) = state.resolutions.get(usize::try_from(index).ok()?)?;
            // Se conserva la frecuencia actual si esa resolución la admite.
            let rates = display.modes.iter().filter(|m| m.width == width && m.height == height).map(|m| m.hz);
            let hz = if rates.clone().any(|hz| hz == display.mode.hz) { display.mode.hz } else { rates.max()? };
            Some((display.id.clone(), display.mode, DisplayMode { width, height, hz }))
        });
        if let Some((id, previous, mode)) = change {
            apply(&app, id, previous, mode);
        }
    });

    let weak = app.as_weak();
    state.on_set_rate(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let change = STATE.with_borrow(|state| {
            let display = state.list.get(state.selected)?;
            let &hz = state.rates.get(usize::try_from(index).ok()?)?;
            Some((display.id.clone(), display.mode, DisplayMode { hz, ..display.mode }))
        });
        if let Some((id, previous, mode)) = change {
            apply(&app, id, previous, mode);
        }
    });

    let weak = app.as_weak();
    state.on_brightness_moved(move |value| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<DisplaysState>().set_brightness(value);
        let Some(id) = STATE.with_borrow(|state| state.list.get(state.selected).map(|d| d.id.clone())) else { return };
        let weak = app.as_weak();
        // Se espera un instante: el monitor responde lento por DDC/CI.
        BRIGHTNESS.with(|timer| {
            timer.start(TimerMode::SingleShot, BRIGHTNESS_DELAY, move || {
                let Some(app) = weak.upgrade() else { return };
                let id = id.clone();
                let level = value.round().clamp(0.0, 100.0) as u32;
                bridge::spawn(
                    &app,
                    move || displays::set_brightness(&id, level),
                    |_, result| {
                        if let Err(err) = result {
                            toasts::error(&err);
                        }
                    },
                );
            });
        });
    });

    let weak = app.as_weak();
    state.on_set_night_light(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<DisplaysState>().set_night_light(on);
        bridge::spawn(
            &app,
            move || displays::set_night_light_enabled(on),
            move |app, result| {
                if let Err(err) = result {
                    app.global::<DisplaysState>().set_night_light(!on);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_night_light_strength_moved(move |value| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<DisplaysState>().set_night_light_strength(value);
        let weak = app.as_weak();
        // Mismo motivo que el brillo: no hace falta escribir el registro en
        // cada pixel que se mueve el deslizador.
        NIGHT_LIGHT.with(|timer| {
            timer.start(TimerMode::SingleShot, BRIGHTNESS_DELAY, move || {
                let Some(app) = weak.upgrade() else { return };
                let percent = value.round().clamp(0.0, 100.0) as u32;
                bridge::spawn(
                    &app,
                    move || displays::set_night_light_strength(percent),
                    |_, result| {
                        if let Err(err) = result {
                            toasts::error(&err);
                        }
                    },
                );
            });
        });
    });

    dialogs::register(CONFIRM_DIALOG, |app, _| keep(app));
    dialogs::register_cancel(CONFIRM_DIALOG, revert);
}

fn load_night_light(app: &AppWindow) {
    bridge::spawn(app, displays::night_light, |app, result| {
        let ui = app.global::<DisplaysState>();
        match result {
            Ok(night) => {
                ui.set_night_light_supported(night.supported);
                ui.set_night_light(night.enabled);
                ui.set_night_light_strength(night.strength as f32);
            }
            Err(_) => ui.set_night_light_supported(false),
        }
    });
}

fn load(app: &AppWindow) {
    bridge::spawn(app, displays::list, |app, list| {
        STATE.with_borrow_mut(|state| {
            state.list = list;
            state.selected = state.selected.min(state.list.len().saturating_sub(1));
        });
        show(app);
        app.global::<DisplaysState>().set_loaded(true);
    });
    load_night_light(app);
}

fn show(app: &AppWindow) {
    let ui = app.global::<DisplaysState>();
    STATE.with_borrow_mut(|state| {
        let (screens, width, height) = arrangement(&state.list);
        ui.set_screens(ModelRc::new(VecModel::from(screens)));
        ui.set_canvas_width(width);
        ui.set_canvas_height(height);
        ui.set_selected(state.selected as i32);
        let Some(display) = state.list.get(state.selected) else { return };

        let mut resolutions: Vec<(u32, u32)> = display.modes.iter().map(|m| (m.width, m.height)).collect();
        resolutions.sort_by_key(|(w, h)| std::cmp::Reverse(u64::from(*w) * u64::from(*h)));
        resolutions.dedup();
        let mut rates: Vec<u32> = display
            .modes
            .iter()
            .filter(|m| m.width == display.mode.width && m.height == display.mode.height)
            .map(|m| m.hz)
            .collect();
        rates.sort_unstable_by(|a, b| b.cmp(a));
        rates.dedup();

        let resolution_labels: Vec<SharedString> =
            resolutions.iter().map(|(w, h)| format!("{w} × {h}").into()).collect();
        let rate_labels: Vec<SharedString> = rates.iter().map(|hz| format!("{hz} Hz").into()).collect();
        let current = (display.mode.width, display.mode.height);
        ui.set_name(display.name.as_str().into());
        ui.set_resolution(resolutions.iter().position(|r| *r == current).map_or(-1, |i| i as i32));
        ui.set_rate(rates.iter().position(|hz| *hz == display.mode.hz).map_or(-1, |i| i as i32));
        ui.set_resolutions(ModelRc::new(VecModel::from(resolution_labels)));
        ui.set_rates(ModelRc::new(VecModel::from(rate_labels)));
        ui.set_scale(display.scale as i32);
        ui.set_has_brightness(display.brightness.is_some());
        ui.set_brightness(display.brightness.unwrap_or(0) as f32);
        ui.set_hdr_supported(display.hdr.is_some());
        ui.set_hdr(display.hdr.unwrap_or(false));
        state.resolutions = resolutions;
        state.rates = rates;
    });
}

/// Monitores escalados para caber en la caja del dibujo.
fn arrangement(list: &[DisplayInfo]) -> (Vec<ScreenView>, f32, f32) {
    if list.is_empty() {
        return (Vec::new(), 0.0, 0.0);
    }
    let min_x = list.iter().map(|d| d.x).min().unwrap_or(0);
    let min_y = list.iter().map(|d| d.y).min().unwrap_or(0);
    let max_x = list.iter().map(|d| d.x + d.mode.width as i32).max().unwrap_or(1);
    let max_y = list.iter().map(|d| d.y + d.mode.height as i32).max().unwrap_or(1);
    let (width, height) = ((max_x - min_x).max(1) as f32, (max_y - min_y).max(1) as f32);
    let scale = (CANVAS.0 / width).min(CANVAS.1 / height);
    let screens = list
        .iter()
        .map(|d| ScreenView {
            name: d.name.as_str().into(),
            primary: d.primary,
            x: (d.x - min_x) as f32 * scale,
            y: (d.y - min_y) as f32 * scale,
            width: d.mode.width as f32 * scale,
            height: d.mode.height as f32 * scale,
        })
        .collect();
    (screens, width * scale, height * scale)
}

fn apply(app: &AppWindow, id: String, previous: DisplayMode, mode: DisplayMode) {
    if previous == mode {
        return;
    }
    let task_id = id.clone();
    bridge::spawn(
        app,
        move || displays::set_mode(&task_id, mode),
        move |app, result| match result {
            Ok(()) => {
                STATE.with_borrow_mut(|state| {
                    if let Some(display) = state.list.iter_mut().find(|d| d.id == id) {
                        display.mode = mode;
                    }
                    state.pending = Some(Pending { id, previous, seconds: CONFIRM_SECONDS });
                });
                show(app);
                ask_to_keep(app);
            }
            Err(err) => toasts::error(&err),
        },
    );
}

fn countdown_text(seconds: i32) -> String {
    format!("Se volverá a la configuración anterior en {seconds} segundos.")
}

/// Pregunta si conservar el modo nuevo; si nadie responde, se revierte solo.
fn ask_to_keep(app: &AppWindow) {
    dialogs::confirm(
        app,
        CONFIRM_DIALOG,
        "¿Conservar esta configuración de pantalla?",
        &countdown_text(CONFIRM_SECONDS),
        "Conservar",
        false,
    );
    dialogs::set_cancel_text(app, "Revertir");
    let weak = app.as_weak();
    COUNTDOWN.with(|timer| {
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let Some(app) = weak.upgrade() else { return };
            let left = STATE.with_borrow_mut(|state| {
                state.pending.as_mut().map(|pending| {
                    pending.seconds -= 1;
                    pending.seconds
                })
            });
            match left {
                Some(seconds) if seconds > 0 => dialogs::set_message(&app, &countdown_text(seconds)),
                _ => revert(&app),
            }
        });
    });
}

fn keep(app: &AppWindow) {
    COUNTDOWN.with(Timer::stop);
    STATE.with_borrow_mut(|state| state.pending = None);
    dialogs::close(app);
}

fn revert(app: &AppWindow) {
    COUNTDOWN.with(Timer::stop);
    dialogs::close(app);
    let Some(pending) = STATE.with_borrow_mut(|state| state.pending.take()) else { return };
    bridge::spawn(
        app,
        move || displays::set_mode(&pending.id, pending.previous),
        |app, result| {
            if let Err(err) = result {
                toasts::error(&err);
            }
            load(app);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{CANVAS, arrangement};
    use liquidcontrol_core::displays::{DisplayInfo, DisplayMode};

    fn display(x: i32, width: u32, height: u32, primary: bool) -> DisplayInfo {
        DisplayInfo {
            id: format!("{x}"),
            name: "Monitor".into(),
            primary,
            x,
            y: 0,
            mode: DisplayMode { width, height, hz: 60 },
            scale: 100,
            modes: Vec::new(),
            brightness: None,
            hdr: None,
        }
    }

    #[test]
    fn arrangement_fits_the_canvas() {
        let (screens, width, height) = arrangement(&[display(0, 2560, 1440, true), display(2560, 1920, 1080, false)]);
        assert_eq!(screens.len(), 2);
        assert!(width <= CANVAS.0 + 0.01 && height <= CANVAS.1 + 0.01);
        assert!(screens[1].x > screens[0].x);
    }
}

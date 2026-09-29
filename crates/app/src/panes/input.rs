//! Mouse y teclado.

use liquidcontrol_core::input::{self, KeyboardSetting, MouseSetting};
use liquidcontrol_core::tweaks::{self, TweakState};
use slint::ComponentHandle;

use super::{Pane, languages};
use crate::{AppWindow, KeyboardState, MouseState, bridge, toasts};

/// El mismo ajuste de Optimización, accesible también desde acá (el pedido
/// más natural: "sacar la demora del mouse" es justamente esto).
const INSTANT_MENUS_ID: &str = "optimize.instant_menus";

/// Los dos paneles comparten `init` (se conecta una sola vez, desde Mouse).
/// Teclado suma los idiomas y distribuciones (ver `languages`).
pub const MOUSE: Pane = Pane::new("mouse", |app, _| load_mouse(app)).with_init(init);
pub const KEYBOARD: Pane = Pane::new("keyboard", |app, _| {
    load_keyboard(app);
    languages::load(app);
})
.with_init(languages::init);

fn init(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<MouseState>().on_set(move |key, value| {
        let Some(app) = weak.upgrade() else { return };
        let Some(setting) = mouse_setting(&key, value) else { return };
        show_mouse_setting(&app, setting);
        bridge::spawn(
            &app,
            move || input::set_mouse(setting),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load_mouse(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    app.global::<MouseState>().on_set_instant_menus(move |enabled| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<MouseState>();
        if state.get_instant_menus_busy() {
            return;
        }
        state.set_instant_menus_busy(true);
        bridge::spawn(
            &app,
            move || tweaks::set(INSTANT_MENUS_ID, enabled),
            |app, result| {
                let state = app.global::<MouseState>();
                state.set_instant_menus_busy(false);
                match result {
                    Ok(tweak) => {
                        state.set_instant_menus(tweak.state == TweakState::On);
                        toasts::show(
                            toasts::Kind::Success,
                            if tweak.state == TweakState::On {
                                "Menús instantáneos. Cerrá sesión para que se note del todo."
                            } else {
                                "Vuelto a la demora normal de los menús."
                            },
                        );
                    }
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });

    let weak = app.as_weak();
    app.global::<KeyboardState>().on_set(move |key, value| {
        let Some(app) = weak.upgrade() else { return };
        let Some(setting) = keyboard_setting(&key, value) else { return };
        show_keyboard_setting(&app, setting);
        bridge::spawn(
            &app,
            move || input::set_keyboard(setting),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load_keyboard(app);
                }
            },
        );
    });
}

pub fn load_mouse(app: &AppWindow) {
    bridge::spawn(app, input::mouse, |app, mouse| {
        let state = app.global::<MouseState>();
        state.set_speed(mouse.speed as i32);
        state.set_precision(mouse.enhance_precision);
        state.set_swap_buttons(mouse.swap_buttons);
        state.set_double_click(mouse.double_click_ms as i32);
        state.set_wheel_lines(mouse.wheel_lines as i32);
        state.set_scroll_inactive(mouse.scroll_inactive);
        state.set_cursor_level(cursor_level(mouse.cursor_size));
        state.set_natural_available(mouse.natural_scroll.is_some());
        state.set_natural_scroll(mouse.natural_scroll.unwrap_or(false));
        state.set_loaded(true);
    });
    bridge::spawn(
        app,
        || tweaks::list("optimize"),
        |app, result| {
            if let Ok(list) = result {
                let on = list.iter().any(|tweak| tweak.id == INSTANT_MENUS_ID && tweak.state == TweakState::On);
                app.global::<MouseState>().set_instant_menus(on);
            }
        },
    );
}

pub fn load_keyboard(app: &AppWindow) {
    bridge::spawn(app, input::keyboard, |app, keyboard| {
        let state = app.global::<KeyboardState>();
        state.set_repeat_rate(keyboard.repeat_rate as i32);
        state.set_repeat_delay(keyboard.repeat_delay as i32);
        state.set_blink(keyboard.caret_blink_ms as i32);
        state.set_caret_width(keyboard.caret_width as i32);
        state.set_loaded(true);
    });
}

fn mouse_setting(key: &str, value: i32) -> Option<MouseSetting> {
    let number = u32::try_from(value).ok()?;
    let on = value != 0;
    Some(match key {
        "speed" => MouseSetting::Speed(number),
        "precision" => MouseSetting::EnhancePrecision(on),
        "swap" => MouseSetting::SwapButtons(on),
        "double-click" => MouseSetting::DoubleClickMs(number),
        "wheel" => MouseSetting::WheelLines(number),
        "scroll-inactive" => MouseSetting::ScrollInactive(on),
        // Niveles 1–15 → 32–256 px en pasos de 16.
        "cursor" => MouseSetting::CursorSize(32 + number.saturating_sub(1) * 16),
        "natural" => MouseSetting::NaturalScroll(on),
        _ => return None,
    })
}

fn show_mouse_setting(app: &AppWindow, setting: MouseSetting) {
    let state = app.global::<MouseState>();
    match setting {
        MouseSetting::Speed(speed) => state.set_speed(speed as i32),
        MouseSetting::EnhancePrecision(on) => state.set_precision(on),
        MouseSetting::SwapButtons(on) => state.set_swap_buttons(on),
        MouseSetting::DoubleClickMs(ms) => state.set_double_click(ms as i32),
        MouseSetting::WheelLines(lines) => state.set_wheel_lines(lines as i32),
        MouseSetting::ScrollInactive(on) => state.set_scroll_inactive(on),
        MouseSetting::CursorSize(size) => state.set_cursor_level(cursor_level(size)),
        MouseSetting::NaturalScroll(on) => state.set_natural_scroll(on),
    }
}

fn cursor_level(size: u32) -> i32 {
    ((size.clamp(32, 256) - 32) / 16 + 1) as i32
}

fn keyboard_setting(key: &str, value: i32) -> Option<KeyboardSetting> {
    let number = u32::try_from(value).ok()?;
    Some(match key {
        "rate" => KeyboardSetting::RepeatRate(number),
        "delay" => KeyboardSetting::RepeatDelay(number),
        "blink" => KeyboardSetting::CaretBlinkMs(number),
        "caret" => KeyboardSetting::CaretWidth(number),
        _ => return None,
    })
}

fn show_keyboard_setting(app: &AppWindow, setting: KeyboardSetting) {
    let state = app.global::<KeyboardState>();
    match setting {
        KeyboardSetting::RepeatRate(rate) => state.set_repeat_rate(rate as i32),
        KeyboardSetting::RepeatDelay(delay) => state.set_repeat_delay(delay as i32),
        KeyboardSetting::CaretBlinkMs(ms) => state.set_blink(ms as i32),
        KeyboardSetting::CaretWidth(width) => state.set_caret_width(width as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_level;

    #[test]
    fn cursor_levels() {
        assert_eq!(cursor_level(32), 1);
        assert_eq!(cursor_level(48), 2);
        assert_eq!(cursor_level(256), 15);
        assert_eq!(cursor_level(10), 1);
    }
}

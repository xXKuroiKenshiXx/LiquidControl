//! Bluetooth: radio y dispositivos emparejados.

use std::cell::Cell;
use std::time::Duration;

use liquidcontrol_core::bluetooth;
use slint::{ComponentHandle, Timer, TimerMode};

use super::Pane;
use crate::{AppWindow, BluetoothDeviceView, BluetoothState, Nav, bridge, models, toasts};

pub const PANE: Pane = Pane::new("bluetooth", |app, _| load(app)).with_init(init);

const REFRESH: Duration = Duration::from_secs(15);

thread_local! {
    static TIMER: Timer = Timer::default();
    /// Mientras se prende o apaga la radio, el refresco periódico se pausa:
    /// si cayera justo en el medio, pisaría el interruptor con el estado
    /// viejo (la radio tarda un momento en reflejar el cambio) y parecería
    /// que el clic no funcionó.
    static CHANGING_RADIO: Cell<bool> = const { Cell::new(false) };
}

pub fn init(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<BluetoothState>().on_set_radio(move |on| {
        let Some(app) = weak.upgrade() else { return };
        CHANGING_RADIO.set(true);
        app.global::<BluetoothState>().set_on(on);
        bridge::spawn(
            &app,
            move || bluetooth::set_radio(on),
            move |app, result| match result {
                Ok(()) => {
                    let weak = app.as_weak();
                    Timer::single_shot(Duration::from_millis(800), move || {
                        CHANGING_RADIO.set(false);
                        if let Some(app) = weak.upgrade() {
                            load(&app);
                        }
                    });
                }
                Err(err) => {
                    CHANGING_RADIO.set(false);
                    app.global::<BluetoothState>().set_on(!on);
                    toasts::error(&err);
                }
            },
        );
    });

    // Los dispositivos se conectan y desconectan solos: se refresca mientras el panel está a la vista.
    let weak = app.as_weak();
    TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, REFRESH, move || {
            let Some(app) = weak.upgrade() else { return };
            if app.global::<Nav>().get_pane() == "bluetooth" && !CHANGING_RADIO.get() {
                load(&app);
            }
        });
    });
}

pub fn load(app: &AppWindow) {
    bridge::spawn(app, bluetooth::status, |app, result| match result {
        Ok(status) => {
            let state = app.global::<BluetoothState>();
            state.set_available(status.available);
            state.set_on(status.on);
            let devices: Vec<BluetoothDeviceView> = status
                .devices
                .iter()
                .map(|device| BluetoothDeviceView {
                    name: device.name.as_str().into(),
                    connected: device.connected,
                    kind: kind(&device.name),
                })
                .collect();
            // Se actualiza fila por fila: el refresco periódico no rearma la lista.
            state.set_devices(models::sync(state.get_devices(), devices));
            state.set_loaded(true);
        }
        Err(err) => toasts::error(&err),
    });
}

/// Tipo aproximado de dispositivo según su nombre (para elegir el ícono).
fn kind(name: &str) -> i32 {
    const KINDS: [(i32, &[&str]); 6] = [
        (1, &["pods", "buds", "headphone", "headset", "auricular", "wh-", "wf-", "beats", "jbl tune"]),
        (2, &["mouse", "mx master", "mx anywhere", "trackball"]),
        (3, &["keyboard", "teclado", "mx keys", "k380"]),
        (4, &["controller", "gamepad", "dualsense", "dualshock", "xbox", "joy-con"]),
        (5, &["iphone", "galaxy", "pixel", "phone", "redmi", "moto"]),
        (6, &["watch", "band"]),
    ];
    let name = name.to_lowercase();
    KINDS.iter().find(|(_, words)| words.iter().any(|word| name.contains(word))).map_or(0, |(kind, _)| *kind)
}

#[cfg(test)]
mod tests {
    use super::kind;

    #[test]
    fn guesses_device_kind() {
        assert_eq!(kind("AirPods Pro"), 1);
        assert_eq!(kind("Sony WH-1000XM5"), 1);
        assert_eq!(kind("MX Master 3S"), 2);
        assert_eq!(kind("Galaxy S23"), 5);
        assert_eq!(kind("Parlante del living"), 0);
    }
}

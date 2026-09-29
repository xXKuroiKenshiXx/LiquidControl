//! Wi‑Fi: radio, conexión actual, redes guardadas y cercanas, y unirse con contraseña.

use std::cell::RefCell;
use std::time::Duration;

use liquidcontrol_core::power;
use liquidcontrol_core::wifi::{self, WifiNetwork, WifiStatus};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::Pane;
use crate::toasts::{self, Kind};
use crate::{AppWindow, Nav, WifiNetworkView, WifiState, bridge, dialogs};

pub const PANE: Pane = Pane::new("wifi", |app, _| enter(app)).with_init(init);

const JOIN_DIALOG: &str = "wifi-join";
const MIN_PASSWORD: i32 = 8;
const REFRESH: Duration = Duration::from_secs(20);

thread_local! {
    /// Red para la que se está pidiendo la contraseña.
    static JOIN_TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
    static TIMER: Timer = Timer::default();
}

pub fn init(app: &AppWindow) {
    let state = app.global::<WifiState>();

    let weak = app.as_weak();
    state.on_set_radio(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<WifiState>().set_radio_on(on);
        bridge::spawn(
            &app,
            move || wifi::set_radio(on),
            move |app, result| match result {
                Ok(()) => {
                    // La radio tarda un momento en encender y ver redes.
                    let weak = app.as_weak();
                    Timer::single_shot(Duration::from_millis(if on { 1500 } else { 300 }), move || {
                        if let Some(app) = weak.upgrade() {
                            refresh(&app);
                        }
                    });
                }
                Err(err) => {
                    app.global::<WifiState>().set_radio_on(!on);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_scan(move || {
        if let Some(app) = weak.upgrade() {
            scan(&app);
        }
    });

    let weak = app.as_weak();
    state.on_join(move |ssid| {
        let Some(app) = weak.upgrade() else { return };
        let needs_password = find(&app, &ssid).is_some_and(|network| network.secured && network.profile.is_empty());
        if needs_password {
            ask_password(&app, &ssid);
        } else {
            connect(&app, ssid.to_string(), None);
        }
    });

    let weak = app.as_weak();
    state.on_forget(move |profile| {
        let Some(app) = weak.upgrade() else { return };
        let profile = profile.to_string();
        let task_profile = profile.clone();
        bridge::spawn(
            &app,
            move || wifi::forget(&task_profile),
            move |app, result| match result {
                Ok(()) => {
                    toasts::show(Kind::Info, format!("Se olvidó la red «{profile}»."));
                    scan(app);
                }
                Err(err) => toasts::error(&err),
            },
        );
    });

    let weak = app.as_weak();
    state.on_disconnect(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(&app, wifi::disconnect, |app, result| match result {
            Ok(()) => refresh(app),
            Err(err) => toasts::error(&err),
        });
    });

    let weak = app.as_weak();
    state.on_set_auto_config(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<WifiState>().set_changing_auto_config(true);
        bridge::spawn(
            &app,
            move || wifi::set_autoconfig(on),
            move |app, result| {
                app.global::<WifiState>().set_changing_auto_config(false);
                match result {
                    Ok(()) => {
                        let message = if on {
                            "Configuración automática de Wi‑Fi activada."
                        } else {
                            "Configuración automática de Wi‑Fi desactivada."
                        };
                        toasts::show(Kind::Success, message);
                        refresh(app);
                    }
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_power_saving(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(level) = u32::try_from(index) else { return };
        app.global::<WifiState>().set_changing_power_saving(true);
        app.global::<WifiState>().set_power_saving(index);
        bridge::spawn(
            &app,
            move || power::set_wifi_power_saving(level),
            move |app, result| {
                app.global::<WifiState>().set_changing_power_saving(false);
                match result {
                    Ok(()) => toasts::show(Kind::Success, "Ahorro de energía del Wi‑Fi actualizado."),
                    Err(err) => {
                        toasts::error(&err);
                        load_power_saving(app);
                    }
                }
            },
        );
    });

    dialogs::register(JOIN_DIALOG, |app, password| {
        let Some(ssid) = JOIN_TARGET.with_borrow(Clone::clone) else {
            dialogs::close(app);
            return;
        };
        dialogs::set_busy(app, true);
        connect(app, ssid, Some(password.to_string()));
    });
    dialogs::register_cancel(JOIN_DIALOG, |app| {
        JOIN_TARGET.set(None);
        dialogs::close(app);
    });

    // Actualiza la lista mientras el panel está a la vista (salvo en medio de una conexión).
    let weak = app.as_weak();
    TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, REFRESH, move || {
            let Some(app) = weak.upgrade() else { return };
            let state = app.global::<WifiState>();
            // Mientras se aplica un cambio (auto-config, ahorro de energía) no se
            // refresca: si el estado real todavía no lo refleja, pisaría el
            // cambio recién hecho y parecería que el clic no funcionó.
            let busy = !state.get_connecting().is_empty()
                || state.get_changing_auto_config()
                || state.get_changing_power_saving()
                || JOIN_TARGET.with_borrow(Option::is_some);
            if app.global::<Nav>().get_pane() == "wifi" && !busy {
                refresh(&app);
            }
        });
    });
}

pub fn enter(app: &AppWindow) {
    refresh(app);
    load_power_saving(app);
}

fn refresh(app: &AppWindow) {
    bridge::spawn(app, wifi::status, |app, result| match result {
        Ok(status) => {
            show_status(app, &status);
            scan(app);
        }
        Err(err) => toasts::error(&err),
    });
}

fn load_power_saving(app: &AppWindow) {
    bridge::spawn(app, power::wifi_power_saving, |app, level| {
        app.global::<WifiState>().set_power_saving(level as i32);
    });
}

fn show_status(app: &AppWindow, status: &WifiStatus) {
    let state = app.global::<WifiState>();
    state.set_adapter(status.adapter.as_deref().unwrap_or_default().into());
    state.set_radio_on(status.radio_on);
    state.set_auto_config(status.auto_config);
    state.set_location_denied(status.location_denied);
    match &status.connection {
        Some(connection) => {
            state.set_connected(true);
            state.set_connection_ssid(connection.ssid.as_str().into());
            state.set_connection_secured(connection.secured);
            let detail = match connection.rx_mbps {
                Some(mbps) => format!("Conectado · {mbps} Mbps"),
                None => "Conectado".into(),
            };
            state.set_connection_detail(detail.into());
        }
        None => state.set_connected(false),
    }
    state.set_loaded(true);
}

fn scan(app: &AppWindow) {
    let state = app.global::<WifiState>();
    let ready = !state.get_adapter().is_empty()
        && state.get_radio_on()
        && !state.get_location_denied()
        && state.get_auto_config();
    if !ready {
        state.set_known(ModelRc::default());
        state.set_others(ModelRc::default());
        return;
    }
    state.set_scanning(true);
    bridge::spawn(app, wifi::scan, |app, result| {
        let state = app.global::<WifiState>();
        state.set_scanning(false);
        match result {
            Ok(list) => {
                let views = |known: bool| -> ModelRc<WifiNetworkView> {
                    let views: Vec<WifiNetworkView> = list
                        .iter()
                        .filter(|network| !network.connected && network.profile.is_some() == known)
                        .map(view)
                        .collect();
                    ModelRc::new(VecModel::from(views))
                };
                state.set_known(views(true));
                state.set_others(views(false));
            }
            Err(err) => match err.code() {
                "location_denied" => state.set_location_denied(true),
                "autoconfig_off" => state.set_auto_config(false),
                _ => toasts::error(&err),
            },
        }
    });
}

fn view(network: &WifiNetwork) -> WifiNetworkView {
    WifiNetworkView {
        ssid: network.ssid.as_str().into(),
        level: level(network.signal),
        secured: network.secured,
        profile: network.profile.as_deref().unwrap_or_default().into(),
    }
}

/// Barras de señal a partir de la calidad (0–100).
fn level(signal: u32) -> i32 {
    match signal {
        67.. => 3,
        34..=66 => 2,
        1..=33 => 1,
        0 => 0,
    }
}

fn find(app: &AppWindow, ssid: &str) -> Option<WifiNetworkView> {
    let state = app.global::<WifiState>();
    [state.get_known(), state.get_others()].into_iter().find_map(|list| list.iter().find(|n| n.ssid == ssid))
}

fn ask_password(app: &AppWindow, ssid: &str) {
    JOIN_TARGET.set(Some(ssid.to_owned()));
    dialogs::ask_secret(
        app,
        JOIN_DIALOG,
        "Unirse a una red Wi‑Fi",
        &format!("Ingresá la contraseña de «{ssid}»."),
        "Contraseña",
        MIN_PASSWORD,
    );
}

/// Conecta con el perfil guardado, a una red abierta o con la contraseña indicada.
/// La contraseña va directo al sistema: nunca se guarda ni se registra.
fn connect(app: &AppWindow, ssid: String, password: Option<String>) {
    app.global::<WifiState>().set_connecting(ssid.as_str().into());
    let task_ssid = ssid.clone();
    bridge::spawn(
        app,
        move || wifi::connect(&task_ssid, password.as_deref()),
        move |app, result| {
            app.global::<WifiState>().set_connecting(SharedString::new());
            let from_dialog = JOIN_TARGET.with_borrow(Option::is_some);
            match result {
                Ok(()) => {
                    if from_dialog {
                        JOIN_TARGET.set(None);
                        dialogs::close(app);
                    }
                    toasts::show(Kind::Success, format!("Conectado a «{ssid}»."));
                    refresh(app);
                }
                Err(err) if from_dialog => dialogs::set_error(app, &err.to_string()),
                Err(err) if err.code() == "needs_password" => ask_password(app, &ssid),
                Err(err) => toasts::error(&err),
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::level;

    #[test]
    fn signal_bars() {
        assert_eq!(level(0), 0);
        assert_eq!(level(20), 1);
        assert_eq!(level(50), 2);
        assert_eq!(level(99), 3);
    }
}

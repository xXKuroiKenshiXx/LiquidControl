//! Sonido: volumen en vivo, silencio, volumen por app (ver `mixer`),
//! dispositivos (tocar uno lo hace predeterminado) y sonidos del sistema.

use std::cell::RefCell;

use liquidcontrol_core::sound::{self, AudioDevice, SoundFlag};
use slint::{ComponentHandle, Model, ModelRc};

use super::{Pane, mixer};
use crate::{AppWindow, DeviceView, SoundState, bridge, models, toasts};

pub const PANE: Pane = Pane::new("sound", |app, _| {
    load(app);
    mixer::enter(app);
})
.with_init(init)
.with_leave(|app, _| mixer::leave(app));

/// El volumen se aplica mientras se arrastra; para no saturar el sistema viaja
/// de a un valor por vez y solo el último que quedó pendiente.
#[derive(Default)]
struct VolumeSync {
    pending: Option<f32>,
    sending: bool,
}

thread_local! {
    static VOLUME: RefCell<VolumeSync> = RefCell::default();
}

fn init(app: &AppWindow) {
    let state = app.global::<SoundState>();

    let weak = app.as_weak();
    state.on_volume_moved(move |value| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<SoundState>();
        state.set_volume(value);
        if state.get_muted() && value > 0.0 {
            set_mute(&app, false);
        }
        push_volume(&app, value);
    });

    let weak = app.as_weak();
    state.on_set_mute(move |muted| {
        if let Some(app) = weak.upgrade() {
            set_mute(&app, muted);
        }
    });

    let weak = app.as_weak();
    state.on_set_default(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<SoundState>();
        // Respuesta inmediata en la lista donde está el dispositivo.
        for model in [state.get_outputs(), state.get_inputs()] {
            if model.iter().any(|device| device.id == id) {
                for (index, mut device) in model.iter().enumerate() {
                    device.is_default = device.id == id;
                    model.set_row_data(index, device);
                }
            }
        }
        let id = id.to_string();
        bridge::spawn(
            &app,
            move || sound::set_default_device(&id),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
                load(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_flag(move |flag, value| {
        let Some(app) = weak.upgrade() else { return };
        let flag = match flag.as_str() {
            "system-sounds" => SoundFlag::SystemSounds,
            "startup-sound" => SoundFlag::StartupSound,
            _ => return,
        };
        let state = app.global::<SoundState>();
        match flag {
            SoundFlag::SystemSounds => state.set_system_sounds(value),
            SoundFlag::StartupSound => state.set_startup_sound(value),
        }
        bridge::spawn(
            &app,
            move || sound::set_flag(flag, value),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load(app);
                }
            },
        );
    });
}

fn load(app: &AppWindow) {
    bridge::spawn(app, sound::status, |app, result| match result {
        Ok(status) => {
            let state = app.global::<SoundState>();
            state.set_volume(status.volume);
            state.set_muted(status.muted);
            state.set_outputs(devices(state.get_outputs(), &status.outputs));
            state.set_inputs(devices(state.get_inputs(), &status.inputs));
            state.set_system_sounds(status.system_sounds);
            state.set_startup_sound(status.startup_sound);
            state.set_loaded(true);
        }
        Err(err) => toasts::error(&err),
    });
}

fn devices(current: ModelRc<DeviceView>, list: &[AudioDevice]) -> ModelRc<DeviceView> {
    let views: Vec<DeviceView> = list
        .iter()
        .map(|device| DeviceView {
            id: device.id.as_str().into(),
            name: device.name.as_str().into(),
            is_default: device.is_default,
        })
        .collect();
    models::sync(current, views)
}

fn set_mute(app: &AppWindow, muted: bool) {
    app.global::<SoundState>().set_muted(muted);
    bridge::spawn(
        app,
        move || sound::set_mute(muted),
        move |app, result| {
            if let Err(err) = result {
                app.global::<SoundState>().set_muted(!muted);
                toasts::error(&err);
            }
        },
    );
}

fn push_volume(app: &AppWindow, value: f32) {
    let start = VOLUME.with_borrow_mut(|sync| {
        sync.pending = Some(value);
        !std::mem::replace(&mut sync.sending, true)
    });
    if start {
        send_next_volume(app);
    }
}

fn send_next_volume(app: &AppWindow) {
    let next = VOLUME.with_borrow_mut(|sync| {
        let next = sync.pending.take();
        sync.sending = next.is_some();
        next
    });
    let Some(value) = next else { return };
    bridge::spawn(
        app,
        move || sound::set_volume(value),
        |app, result| {
            if let Err(err) = result {
                toasts::error(&err);
            }
            send_next_volume(app);
        },
    );
}

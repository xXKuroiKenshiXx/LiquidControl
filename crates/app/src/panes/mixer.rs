//! Mezclador de volumen por app (sección del panel Sonido): las apps con el
//! audio abierto, con su ícono, volumen y silencio propios. La lista se refresca
//! sola mientras el panel está a la vista.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use liquidcontrol_core::mixer::{self, AudioApp};
use slint::{ComponentHandle, Image, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::images::{self, Source};
use crate::{AppWindow, MixerAppView, MixerState, Nav, bridge, format, toasts};

const REFRESH: Duration = Duration::from_secs(3);
const ICON_SIZE: u32 = 32;

static ICON_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Igual que el volumen general: viaja de a un valor por vez y solo el último pendiente.
#[derive(Default)]
struct VolumeSync {
    pending: Option<(String, f32)>,
    sending: bool,
}

thread_local! {
    static TIMER: Timer = Timer::default();
    static VOLUME: RefCell<VolumeSync> = RefCell::default();
    /// Íconos ya cargados por app: no se vuelven a pedir en cada refresco.
    static ICONS: RefCell<HashMap<String, Image>> = RefCell::default();
    /// Mientras se arrastra un deslizador no se refresca (el valor saltaría).
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
}

pub fn init(app: &AppWindow) {
    let state = app.global::<MixerState>();

    let weak = app.as_weak();
    state.on_volume_moved(move |key, value| {
        let Some(app) = weak.upgrade() else { return };
        DRAGGING.set(true);
        update(&app, &key, |item| item.volume = value);
        push_volume(&app, key.to_string(), value);
    });

    let weak = app.as_weak();
    state.on_volume_released(move |key, value| {
        let Some(app) = weak.upgrade() else { return };
        DRAGGING.set(false);
        update(&app, &key, |item| item.volume = value);
        push_volume(&app, key.to_string(), value);
    });

    let weak = app.as_weak();
    state.on_set_mute(move |key, muted| {
        let Some(app) = weak.upgrade() else { return };
        update(&app, &key, |item| item.muted = muted);
        let key = key.to_string();
        let task_key = key.clone();
        bridge::spawn(
            &app,
            move || mixer::set_mute(&task_key, muted),
            move |app, result| {
                if let Err(err) = result {
                    update(app, &key, |item| item.muted = !muted);
                    toasts::error(&err);
                }
            },
        );
    });
}

/// Al entrar a Sonido: carga la lista y la mantiene al día.
pub fn enter(app: &AppWindow) {
    refresh(app, true);
    let weak = app.as_weak();
    TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, REFRESH, move || {
            let Some(app) = weak.upgrade() else { return };
            if app.global::<Nav>().get_pane() == "sound" && !DRAGGING.get() {
                refresh(&app, false);
            }
        });
    });
}

/// Al salir de Sonido: se detiene el refresco y se sueltan los íconos.
pub fn leave(app: &AppWindow) {
    TIMER.with(Timer::stop);
    ICON_GENERATION.fetch_add(1, Ordering::SeqCst);
    ICONS.with_borrow_mut(HashMap::clear);
    DRAGGING.set(false);
    let state = app.global::<MixerState>();
    state.set_apps(ModelRc::default());
    state.set_loaded(false);
}

/// `report`: los errores del refresco automático no se muestran cada 3 segundos.
fn refresh(app: &AppWindow, report: bool) {
    bridge::spawn(app, mixer::apps, move |app, result| match result {
        Ok(list) => show(app, &list),
        Err(err) if report => toasts::error(&err),
        Err(_) => {}
    });
}

fn show(app: &AppWindow, list: &[AudioApp]) {
    let state = app.global::<MixerState>();
    let views: Vec<MixerAppView> = list.iter().map(view).collect();
    let model = state.get_apps();
    let same_apps = model.row_count() == views.len() && model.iter().zip(&views).all(|(old, new)| old.key == new.key);
    if same_apps {
        // Misma lista: solo cambian los valores (no se rearman las filas).
        for (index, view) in views.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&view) {
                model.set_row_data(index, view);
            }
        }
    } else {
        state.set_apps(ModelRc::new(VecModel::from(views)));
        load_icons(app, list);
    }
    state.set_loaded(true);
}

fn view(item: &AudioApp) -> MixerAppView {
    let icon = ICONS.with_borrow(|icons| icons.get(&item.key).cloned());
    MixerAppView {
        key: item.key.as_str().into(),
        name: item.name.as_str().into(),
        letter: format::initial(&item.name).into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        tint: format::tint_index(&item.key),
        volume: item.volume,
        muted: item.muted,
        active: item.active,
    }
}

fn load_icons(app: &AppWindow, list: &[AudioApp]) {
    let jobs: Vec<(String, Source)> = list
        .iter()
        .filter(|item| !ICONS.with_borrow(|icons| icons.contains_key(&item.key)))
        .filter_map(|item| {
            let exe = if item.system { system_sounds_icon() } else { item.exe.clone() }?;
            Some((item.key.clone(), Source::FileIcon(exe)))
        })
        .collect();
    images::load(app, jobs, ICON_SIZE, &ICON_GENERATION, set_icon);
}

/// Los sonidos del sistema llevan el ícono del mezclador de Windows.
fn system_sounds_icon() -> Option<PathBuf> {
    std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join(r"System32\SndVol.exe"))
}

fn set_icon(app: &AppWindow, key: String, pixels: images::Pixels) {
    let image = images::image(pixels);
    update(app, &key, |item| {
        item.icon = image.clone();
        item.has_icon = true;
    });
    ICONS.with_borrow_mut(|icons| icons.insert(key, image));
}

fn update(app: &AppWindow, key: &str, change: impl FnOnce(&mut MixerAppView)) {
    let model = app.global::<MixerState>().get_apps();
    if let Some((index, mut item)) = model.iter().enumerate().find(|(_, item)| item.key == key) {
        change(&mut item);
        model.set_row_data(index, item);
    }
}

fn push_volume(app: &AppWindow, key: String, value: f32) {
    let start = VOLUME.with_borrow_mut(|sync| {
        sync.pending = Some((key, value));
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
    let Some((key, value)) = next else { return };
    bridge::spawn(
        app,
        move || mixer::set_volume(&key, value),
        |app, result| {
            if let Err(err) = result {
                toasts::error(&err);
            }
            send_next_volume(app);
        },
    );
}

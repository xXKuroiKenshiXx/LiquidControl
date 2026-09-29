//! Notificaciones: avisos globales y permiso por app (con el ícono de cada una),
//! con filtro.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use liquidcontrol_core::notifications::{self, NotificationFlag};
use slint::{ComponentHandle, Image, Model, ModelRc};

use super::Pane;
use crate::images::{self, Source};
use crate::{AppWindow, NotificationAppView, NotificationsState, bridge, format, models, search, toasts};

pub const PANE: Pane = Pane::new("notifications", |app, _| load(app)).with_init(init).with_leave(|app, _| leave(app));

const ICON_SIZE: u32 = 32;

static ICON_GENERATION: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Todas las apps; la interfaz recibe solo las que pasan el filtro.
    static APPS: RefCell<Vec<NotificationAppView>> = RefCell::default();
}

fn init(app: &AppWindow) {
    let state = app.global::<NotificationsState>();

    let weak = app.as_weak();
    state.on_set_flag(move |flag, value| {
        let Some(app) = weak.upgrade() else { return };
        let Some(flag) = parse_flag(&flag) else { return };
        show_flag(&app, flag, value);
        bridge::spawn(
            &app,
            move || notifications::set_flag(flag, value),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_app(move |id, enabled| {
        let Some(app) = weak.upgrade() else { return };
        set_app_local(&app, &id, enabled);
        let id = id.to_string();
        let task_id = id.clone();
        bridge::spawn(
            &app,
            move || notifications::set_app(&task_id, enabled),
            move |app, result| {
                if let Err(err) = result {
                    set_app_local(app, &id, !enabled);
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_filter_changed(move |_| {
        if let Some(app) = weak.upgrade() {
            refresh_visible(&app);
        }
    });
}

fn load(app: &AppWindow) {
    bridge::spawn(app, notifications::get, |app, result| match result {
        Ok(data) => {
            let state = app.global::<NotificationsState>();
            state.set_enabled(data.enabled);
            state.set_lock_screen(data.lock_screen);
            state.set_sounds(data.sounds);
            state.set_tips(data.tips);
            let views = data
                .apps
                .iter()
                .map(|item| NotificationAppView {
                    id: item.id.as_str().into(),
                    name: item.name.as_str().into(),
                    enabled: item.enabled,
                    tint: format::tint_index(&item.id),
                    letter: format::initial(&item.name).into(),
                    icon: Image::default(),
                    has_icon: false,
                })
                .collect();
            APPS.set(views);
            refresh_visible(app);
            state.set_loaded(true);
            // Los ids de notificación son AppUserModelIDs: Windows entrega el ícono de cada app.
            let jobs = data.apps.into_iter().map(|item| (item.id.clone(), Source::AppIcon(item.id))).collect();
            images::load(app, jobs, ICON_SIZE, &ICON_GENERATION, set_icon);
        }
        Err(err) => toasts::error(&err),
    });
}

/// Al salir del panel se sueltan la lista y los íconos.
fn leave(app: &AppWindow) {
    ICON_GENERATION.fetch_add(1, Ordering::SeqCst);
    APPS.set(Vec::new());
    app.global::<NotificationsState>().set_apps(ModelRc::default());
}

fn set_icon(app: &AppWindow, id: String, pixels: images::Pixels) {
    let image = images::image(pixels);
    APPS.with_borrow_mut(|apps| {
        if let Some(item) = apps.iter_mut().find(|item| item.id == id.as_str()) {
            item.icon = image.clone();
            item.has_icon = true;
        }
    });
    let model = app.global::<NotificationsState>().get_apps();
    if let Some((index, mut item)) = model.iter().enumerate().find(|(_, item)| item.id == id.as_str()) {
        item.icon = image;
        item.has_icon = true;
        model.set_row_data(index, item);
    }
}

fn parse_flag(name: &str) -> Option<NotificationFlag> {
    Some(match name {
        "enabled" => NotificationFlag::Enabled,
        "lock-screen" => NotificationFlag::LockScreen,
        "sounds" => NotificationFlag::Sounds,
        "tips" => NotificationFlag::Tips,
        _ => return None,
    })
}

fn show_flag(app: &AppWindow, flag: NotificationFlag, value: bool) {
    let state = app.global::<NotificationsState>();
    match flag {
        NotificationFlag::Enabled => state.set_enabled(value),
        NotificationFlag::LockScreen => state.set_lock_screen(value),
        NotificationFlag::Sounds => state.set_sounds(value),
        NotificationFlag::Tips => state.set_tips(value),
    }
}

fn set_app_local(app: &AppWindow, id: &str, enabled: bool) {
    APPS.with_borrow_mut(|apps| {
        if let Some(item) = apps.iter_mut().find(|item| item.id == id) {
            item.enabled = enabled;
        }
    });
    refresh_visible(app);
}

fn refresh_visible(app: &AppWindow) {
    let state = app.global::<NotificationsState>();
    let filter = search::normalize(state.get_filter().trim());
    let visible: Vec<NotificationAppView> = APPS.with_borrow(|apps| {
        apps.iter()
            .filter(|item| filter.is_empty() || search::normalize(&item.name).contains(&filter))
            .cloned()
            .collect()
    });
    state.set_apps(models::sync(state.get_apps(), visible));
}

//! General: información del equipo, almacenamiento e ítems de inicio (con
//! íconos, quitar con copia de respaldo y restaurar).

use std::cell::RefCell;
use std::sync::atomic::AtomicU32;

use liquidcontrol_core::system::{self, DriveKind, RemovedStartup, StartupItem, SystemInfo};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::tweaks::{self, Category};
use super::{Pane, fixed_target, fixed_title};
use crate::images::{self, Source};
use crate::toasts::{self, Kind};
use crate::{AppWindow, DriveView, GeneralState, RemovedStartupView, StartupView, bridge, dialogs, format, models};

pub const PANE: Pane = Pane::new("general", enter).with_init(init).with_subpages(sub_title, target);

const SUBPAGES: [(&str, &str); 4] = [
    ("about", "Información"),
    ("storage", "Almacenamiento"),
    ("startup", "Ítems de inicio"),
    ("updates", "Actualizaciones de Windows"),
];
const REMOVE_DIALOG: &str = "startup-remove";
const ICON_SIZE: u32 = 32;

static ICON_GENERATION: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Ítem que se está por quitar: (id, nombre).
    static TO_REMOVE: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

fn init(app: &AppWindow) {
    let state = app.global::<GeneralState>();

    let weak = app.as_weak();
    state.on_set_startup(move |id, enabled| {
        let Some(app) = weak.upgrade() else { return };
        // Respuesta inmediata en la interfaz; si falla, se recarga el estado real.
        set_startup_local(&app, &id, enabled);
        let id = id.to_string();
        bridge::spawn(
            &app,
            move || system::set_startup_item(&id, enabled),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load_startup(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_remove_startup(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let model = app.global::<GeneralState>().get_startup();
        let Some(name) = model.iter().find(|item| item.id == id).map(|item| item.name.to_string()) else { return };
        TO_REMOVE.set(Some((id.to_string(), name.clone())));
        dialogs::confirm(
            &app,
            REMOVE_DIALOG,
            &format!("¿Quitar «{name}» del inicio?"),
            "Deja de abrirse al iniciar sesión y sale de la lista. Podés restaurarlo después desde «Quitados recientemente».",
            "Quitar",
            true,
        );
    });
    dialogs::register(REMOVE_DIALOG, |app, _| {
        let Some((id, name)) = TO_REMOVE.take() else {
            dialogs::close(app);
            return;
        };
        dialogs::set_busy(app, true);
        bridge::spawn(
            app,
            move || system::remove_startup_item(&id),
            move |app, result| {
                dialogs::close(app);
                match result {
                    Ok(()) => toasts::show(Kind::Success, format!("Se quitó «{name}» del inicio.")),
                    Err(err) => toasts::error(&err),
                }
                load_startup(app);
            },
        );
    });
    dialogs::register_cancel(REMOVE_DIALOG, |app| {
        TO_REMOVE.set(None);
        dialogs::close(app);
    });

    let weak = app.as_weak();
    state.on_restore_startup(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(index) = usize::try_from(index) else { return };
        bridge::spawn(
            &app,
            move || system::restore_startup_item(index),
            |app, result| {
                match result {
                    Ok(()) => toasts::show(Kind::Success, "Se restauró el ítem de inicio."),
                    Err(err) => toasts::error(&err),
                }
                load_startup(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_check_for_updates(move || {
        if let Some(app) = weak.upgrade() {
            on_check_for_updates(&app);
        }
    });
}

fn on_check_for_updates(app: &AppWindow) {
    let state = app.global::<GeneralState>();
    if state.get_checking_updates() {
        return;
    }
    state.set_checking_updates(true);
    bridge::spawn(app, system::check_for_updates, |app, result| {
        app.global::<GeneralState>().set_checking_updates(false);
        match result {
            Ok(()) => toasts::show(Kind::Info, "Windows está buscando actualizaciones…"),
            Err(err) => toasts::error(&err),
        }
    });
}

fn sub_title(sub: &str) -> Option<String> {
    fixed_title(&SUBPAGES, sub)
}

fn target(item: &str) -> (String, String) {
    fixed_target(&SUBPAGES, item)
}

fn enter(app: &AppWindow, sub: &str) {
    match sub {
        "about" => load_info(app),
        "storage" => load_storage(app),
        "startup" => load_startup(app),
        "updates" => tweaks::load(app, Category::Updates),
        _ => {}
    }
}

fn load_info(app: &AppWindow) {
    bridge::spawn(app, system::info, |app, result| match result {
        Ok(info) => show_info(app, &info),
        Err(err) => toasts::error(&err),
    });
}

fn show_info(app: &AppWindow, info: &SystemInfo) {
    let state = app.global::<GeneralState>();
    let model = [info.manufacturer.as_str(), info.model.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    state.set_device_name(info.device_name.as_str().into());
    state.set_device_model(if model.is_empty() { info.os.name.as_str().into() } else { model.into() });
    state.set_cpu(info.cpu.as_str().into());
    state.set_cores(format!("{} núcleos · {} hilos", info.cpu_cores, info.cpu_threads).into());
    state.set_memory(format::bytes(info.ram_total).into());
    let gpus: Vec<SharedString> = info
        .gpus
        .iter()
        .map(|gpu| match gpu.vram_bytes {
            0 => gpu.name.as_str().into(),
            vram => format!("{} ({})", gpu.name, format::bytes(vram)).into(),
        })
        .collect();
    state.set_gpus(ModelRc::new(VecModel::from(gpus)));
    state.set_windows_name(info.os.name.as_str().into());
    state.set_windows_version(info.os.display_version.as_str().into());
    state.set_build(format!("{}.{}", info.os.build, info.os.ubr).into());
    state.set_uptime(format::duration(info.uptime_secs).into());
    state.set_info_loaded(true);
}

fn load_storage(app: &AppWindow) {
    bridge::spawn(app, system::storage, |app, drives| {
        let views: Vec<DriveView> = drives
            .iter()
            .map(|drive| {
                let letter = drive.root.trim_end_matches('\\');
                let kind = match drive.kind {
                    DriveKind::Fixed => "Disco local",
                    DriveKind::Removable => "Unidad extraíble",
                    DriveKind::CdRom => "Unidad de CD/DVD",
                    DriveKind::Other => "Unidad",
                };
                let name = if drive.label.is_empty() { kind } else { drive.label.as_str() };
                let used = drive.total.saturating_sub(drive.free);
                DriveView {
                    name: format!("{name} ({letter})").into(),
                    detail: format!("{} disponibles de {}", format::bytes(drive.free), format::bytes(drive.total))
                        .into(),
                    file_system: drive.file_system.as_str().into(),
                    used: format!("Usado: {}", format::bytes(used)).into(),
                    free: format!("Disponible: {}", format::bytes(drive.free)).into(),
                    ratio: if drive.total > 0 { (used as f64 / drive.total as f64) as f32 } else { 0.0 },
                    removable: drive.kind == DriveKind::Removable,
                }
            })
            .collect();
        app.global::<GeneralState>().set_drives(ModelRc::new(VecModel::from(views)));
    });
}

fn load_startup(app: &AppWindow) {
    bridge::spawn(
        app,
        || (system::startup_items(), system::removed_startup_items()),
        |app, (items, removed)| show_startup(app, items, &removed),
    );
}

fn show_startup(app: &AppWindow, items: Vec<StartupItem>, removed: &[RemovedStartup]) {
    let state = app.global::<GeneralState>();
    let old = state.get_startup();
    let views: Vec<StartupView> = items
        .iter()
        .map(|item| {
            // El ícono ya cargado se conserva: al recargar la lista no parpadea.
            let previous = old.iter().find(|row| row.id == item.id.as_str() && row.has_icon);
            StartupView {
                id: item.id.as_str().into(),
                name: item.name.as_str().into(),
                detail: item.source.label().into(),
                enabled: item.enabled,
                admin: item.source.needs_admin(),
                letter: format::initial(&item.name).into(),
                has_icon: previous.is_some(),
                icon: previous.map_or_else(Image::default, |row| row.icon),
                tint: format::tint_index(&item.name),
            }
        })
        .collect();
    let removed_views: Vec<RemovedStartupView> = removed
        .iter()
        .enumerate()
        .rev()
        .map(|(index, entry)| RemovedStartupView {
            index: index as i32,
            name: entry.name.trim_end_matches(".lnk").into(),
            detail: format!("{} · {}", entry.source.label(), entry.command).into(),
        })
        .collect();
    state.set_startup_needs_admin(items.iter().any(|item| item.source.needs_admin()));
    let missing: Vec<bool> = views.iter().map(|view| !view.has_icon).collect();
    state.set_startup(models::sync(old, views));
    state.set_removed(ModelRc::new(VecModel::from(removed_views)));
    update_summary(app);
    let jobs = items
        .into_iter()
        .enumerate()
        .filter(|(index, _)| missing[*index])
        .filter_map(|(index, item)| item.exe.map(|exe| (index, Source::FileIcon(exe))))
        .collect();
    images::load(app, jobs, ICON_SIZE, &ICON_GENERATION, set_icon);
}

fn set_icon(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let model = app.global::<GeneralState>().get_startup();
    if let Some(mut item) = model.row_data(index) {
        item.icon = images::image(pixels);
        item.has_icon = true;
        model.set_row_data(index, item);
    }
}

fn set_startup_local(app: &AppWindow, id: &str, enabled: bool) {
    let model = app.global::<GeneralState>().get_startup();
    if let Some((index, mut item)) = model.iter().enumerate().find(|(_, item)| item.id == id) {
        item.enabled = enabled;
        model.set_row_data(index, item);
    }
    update_summary(app);
}

fn update_summary(app: &AppWindow) {
    let state = app.global::<GeneralState>();
    let model = state.get_startup();
    let total = model.row_count();
    let enabled = model.iter().filter(|item| item.enabled).count();
    let summary = if total == 0 { String::new() } else { format!("{enabled} de {total} se abren al iniciar sesión.") };
    state.set_startup_summary(summary.into());
}

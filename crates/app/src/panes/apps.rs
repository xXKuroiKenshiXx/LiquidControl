//! Aplicaciones: programas instalados (con su ícono; buscar, ordenar y
//! desinstalar a fondo) y catálogo curado para instalar con winget, con el ícono
//! público del sitio oficial de cada app.
//!
//! Desinstalación profunda: se abre el desinstalador de la app y, cuando
//! termina (la app desaparece de las instaladas), se buscan los restos (ver
//! `liquidcontrol_core::leftovers`) para que el usuario elija qué borrar.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use liquidcontrol_core::apps::{self, InstalledApp};
use liquidcontrol_core::leftovers::{self, Leftover, LeftoverKind, Target};
use liquidcontrol_core::{AppError, icons};
use slint::{ComponentHandle, Image, Model, ModelRc, Timer, TimerMode, VecModel};

use super::{Pane, fixed_target, fixed_title};
use crate::images::{self, Source};
use crate::toasts::{self, Kind};
use crate::{
    AppWindow, AppsState, CatalogAppView, CatalogGroupView, InstalledAppView, LeftoverView, bridge, dialogs, format,
    models, nav, prefs, search,
};

pub const PANE: Pane = Pane::new("apps", enter).with_init(init).with_leave(leave).with_subpages(sub_title, target);

const SUBPAGES: [(&str, &str); 1] = [("cleanup", "Desinstalación profunda")];
const UNINSTALL_DIALOG: &str = "app-uninstall";
const CLEAN_DIALOG: &str = "app-clean";
const ICON_SIZE: u32 = 40;
/// Cada cuánto se revisa si el desinstalador ya terminó.
const POLL: Duration = Duration::from_secs(2);
/// Se deja de revisar solo a los 30 minutos (queda el botón "Buscar restos ahora").
const POLL_LIMIT: u32 = 900;

static INSTALLED_GENERATION: AtomicU32 = AtomicU32::new(0);
static CATALOG_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Desinstalación en curso.
struct Cleanup {
    id: String,
    target: Target,
    polls: u32,
}

thread_local! {
    static INSTALLED: RefCell<Vec<InstalledApp>> = RefCell::default();
    /// Íconos de las instaladas por id (la lista visible se rearma al buscar u ordenar).
    static ICONS: RefCell<HashMap<String, Image>> = RefCell::default();
    static CATALOG_ICONS_REQUESTED: Cell<bool> = const { Cell::new(false) };
    /// App elegida para desinstalar: (id, nombre).
    static UNINSTALL_TARGET: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    static CLEANUP: RefCell<Option<Cleanup>> = const { RefCell::new(None) };
    static POLL_TIMER: Timer = Timer::default();
}

fn init(app: &AppWindow) {
    let state = app.global::<AppsState>();
    match apps::winget_catalog() {
        Ok(groups) => {
            let views: Vec<CatalogGroupView> = groups
                .iter()
                .map(|group| {
                    let items: Vec<CatalogAppView> = group
                        .apps
                        .iter()
                        .map(|item| CatalogAppView {
                            id: item.id.as_str().into(),
                            name: item.name.as_str().into(),
                            description: item.description.as_str().into(),
                            tint: format::tint_index(&item.id),
                            letter: format::initial(&item.name).into(),
                            icon: Image::default(),
                            has_icon: false,
                            store: item.source == "msstore",
                            installed: false,
                            busy: false,
                        })
                        .collect();
                    CatalogGroupView { title: group.title.as_str().into(), apps: ModelRc::new(VecModel::from(items)) }
                })
                .collect();
            state.set_catalog(ModelRc::new(VecModel::from(views)));
        }
        Err(err) => log::error!("{err}"),
    }

    let weak = app.as_weak();
    state.on_set_tab(move |tab| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<AppsState>();
        state.set_tab(tab);
        if tab == 1 {
            load_catalog_icons(&app);
            if !state.get_winget_loaded() {
                load_winget(&app);
            }
        }
    });

    let weak = app.as_weak();
    state.on_set_sort(move |sort| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<AppsState>().set_sort(sort);
        refresh_visible(&app);
    });

    let weak = app.as_weak();
    state.on_query_changed(move |_| {
        if let Some(app) = weak.upgrade() {
            refresh_visible(&app);
        }
    });

    init_uninstall(app);
    init_cleanup(app);

    let weak = app.as_weak();
    state.on_install(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(name) = update_catalog(&app, &id, |item| {
            item.busy = true;
            item.name.to_string()
        }) else {
            return;
        };
        let id = id.to_string();
        let task_id = id.clone();
        bridge::spawn(
            &app,
            move || apps::winget_install(&task_id),
            move |app, result| {
                let ok = result.is_ok();
                update_catalog(app, &id, |item| {
                    item.busy = false;
                    item.installed |= ok;
                });
                match result {
                    Ok(()) => toasts::show(Kind::Success, format!("{name} se instaló correctamente.")),
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });
}

/// Desinstalar: confirmación, desinstalador de la app y, después, los restos.
fn init_uninstall(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<AppsState>().on_uninstall(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(name) =
            INSTALLED.with_borrow(|list| list.iter().find(|item| item.id == id.as_str()).map(|item| item.name.clone()))
        else {
            return;
        };
        dialogs::confirm(
            &app,
            UNINSTALL_DIALOG,
            &format!("¿Desinstalar {name}?"),
            "Se abre el desinstalador de la app. Cuando termine, LiquidControl busca lo que haya quedado (carpetas, accesos directos y registro) para que elijas qué borrar.",
            "Desinstalar",
            true,
        );
        UNINSTALL_TARGET.set(Some((id.to_string(), name)));
    });
    dialogs::register(UNINSTALL_DIALOG, |app, _| {
        let Some((id, _)) = UNINSTALL_TARGET.take() else {
            dialogs::close(app);
            return;
        };
        // Los datos de la app se toman antes: después del desinstalador ya no están en el registro.
        let Some(target) = INSTALLED.with_borrow(|list| list.iter().find(|item| item.id == id).map(target_of)) else {
            dialogs::close(app);
            return;
        };
        dialogs::set_busy(app, true);
        let task_id = id.clone();
        bridge::spawn(
            app,
            move || apps::uninstall(&task_id),
            move |app, result| {
                dialogs::close(app);
                match result {
                    Ok(()) => start_cleanup(app, id, target),
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });
    dialogs::register_cancel(UNINSTALL_DIALOG, |app| {
        UNINSTALL_TARGET.set(None);
        dialogs::close(app);
    });
}

/// Restos: buscar, marcar y eliminar.
fn init_cleanup(app: &AppWindow) {
    let state = app.global::<AppsState>();

    let weak = app.as_weak();
    state.on_scan_leftovers(move || {
        if let Some(app) = weak.upgrade() {
            POLL_TIMER.with(Timer::stop);
            scan(&app);
        }
    });

    let weak = app.as_weak();
    state.on_toggle_leftover(move |id, checked| {
        if let Some(app) = weak.upgrade() {
            update_leftovers(&app, |item| {
                if item.id == id {
                    item.checked = checked;
                }
            });
        }
    });

    let weak = app.as_weak();
    state.on_select_all_leftovers(move |checked| {
        if let Some(app) = weak.upgrade() {
            update_leftovers(&app, |item| item.checked = checked);
        }
    });

    let weak = app.as_weak();
    state.on_clean_leftovers(move || {
        let Some(app) = weak.upgrade() else { return };
        let count = chosen_ids(&app).len();
        if count == 0 {
            return;
        }
        dialogs::confirm(
            &app,
            CLEAN_DIALOG,
            &format!("¿Eliminar {count} restos?"),
            "Los archivos y carpetas van a la Papelera. Antes de borrar cada clave del registro se guarda una copia (.reg) en la carpeta de datos de LiquidControl.",
            "Eliminar",
            true,
        );
    });
    dialogs::register(CLEAN_DIALOG, |app, _| {
        let ids = chosen_ids(app);
        dialogs::close(app);
        app.global::<AppsState>().set_cleanup_stage(3);
        bridge::spawn(
            app,
            move || leftovers::remove(&ids),
            |app, report| {
                if report.removed > 0 {
                    toasts::show(Kind::Success, format!("Se eliminaron {} restos.", report.removed));
                }
                if report.needs_admin {
                    toasts::error(&AppError::NeedsAdmin);
                } else if !report.failed.is_empty() {
                    toasts::show(Kind::Error, format!("No se pudieron eliminar {} restos.", report.failed.len()));
                }
                scan(app);
            },
        );
    });
}

fn target_of(item: &InstalledApp) -> Target {
    Target {
        name: item.name.clone(),
        publisher: item.publisher.clone(),
        install_location: item.install_location.clone(),
        registry_key: item.registry_key.clone(),
    }
}

/// Abre la subpágina y revisa cada 2 s si el desinstalador terminó; cuando la
/// app ya no figura como instalada, busca los restos solo.
fn start_cleanup(app: &AppWindow, id: String, target: Target) {
    let state = app.global::<AppsState>();
    state.set_cleanup_name(target.name.as_str().into());
    state.set_cleanup_stage(0);
    state.set_leftovers(ModelRc::default());
    state.set_leftovers_selected(0);
    CLEANUP.set(Some(Cleanup { id, target, polls: 0 }));
    nav::open(app, "apps", "cleanup", "");
    let weak = app.as_weak();
    POLL_TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, POLL, move || {
            let Some(app) = weak.upgrade() else { return };
            let next = CLEANUP.with_borrow_mut(|cleanup| {
                cleanup.as_mut().map(|cleanup| {
                    cleanup.polls += 1;
                    (cleanup.id.clone(), cleanup.polls)
                })
            });
            let Some((id, polls)) = next.filter(|(_, polls)| *polls <= POLL_LIMIT) else {
                POLL_TIMER.with(Timer::stop);
                return;
            };
            let _ = polls;
            bridge::spawn(
                &app,
                move || apps::is_installed(&id),
                |app, installed| {
                    if !installed && app.global::<AppsState>().get_cleanup_stage() == 0 {
                        POLL_TIMER.with(Timer::stop);
                        scan(app);
                    }
                },
            );
        });
    });
}

fn scan(app: &AppWindow) {
    let Some(target) = CLEANUP.with_borrow(|cleanup| cleanup.as_ref().map(|cleanup| cleanup.target.clone())) else {
        return;
    };
    app.global::<AppsState>().set_cleanup_stage(1);
    bridge::spawn(app, move || leftovers::scan(&target), |app, items| show_leftovers(app, &items));
}

fn show_leftovers(app: &AppWindow, items: &[Leftover]) {
    let views: Vec<LeftoverView> = items
        .iter()
        .map(|item| LeftoverView {
            id: item.id as i32,
            kind: match item.kind {
                LeftoverKind::Folder => 0,
                LeftoverKind::File => 1,
                LeftoverKind::RegistryKey => 2,
            },
            path: item.path.as_str().into(),
            detail: if item.size > 0 {
                format!("{} · {}", item.reason, format::bytes(item.size))
            } else {
                item.reason.clone()
            }
            .into(),
            checked: item.recommended,
        })
        .collect();
    let state = app.global::<AppsState>();
    state.set_leftovers(ModelRc::new(VecModel::from(views)));
    state.set_cleanup_stage(2);
    update_cleanup_summary(app);
}

fn update_leftovers(app: &AppWindow, change: impl Fn(&mut LeftoverView)) {
    let model = app.global::<AppsState>().get_leftovers();
    for (index, item) in model.iter().enumerate() {
        let mut changed = item.clone();
        change(&mut changed);
        if changed != item {
            model.set_row_data(index, changed);
        }
    }
    update_cleanup_summary(app);
}

fn update_cleanup_summary(app: &AppWindow) {
    let state = app.global::<AppsState>();
    let model = state.get_leftovers();
    let total = model.row_count();
    let chosen = model.iter().filter(|item| item.checked).count();
    state.set_leftovers_selected(chosen as i32);
    state.set_cleanup_summary(
        match total {
            0 => "Sin restos".to_owned(),
            1 => format!("1 resto encontrado · {chosen} marcado"),
            _ => format!("{total} restos encontrados · {chosen} marcados"),
        }
        .into(),
    );
}

fn chosen_ids(app: &AppWindow) -> Vec<usize> {
    app.global::<AppsState>()
        .get_leftovers()
        .iter()
        .filter(|item| item.checked)
        .filter_map(|item| usize::try_from(item.id).ok())
        .collect()
}

fn sub_title(sub: &str) -> Option<String> {
    fixed_title(&SUBPAGES, sub)
}

fn target(item: &str) -> (String, String) {
    fixed_target(&SUBPAGES, item)
}

fn enter(app: &AppWindow, sub: &str) {
    if sub == "cleanup" {
        return;
    }
    load_installed(app);
    if app.global::<AppsState>().get_tab() == 1 {
        load_catalog_icons(app);
        load_winget(app);
    }
}

/// Al salir se sueltan las listas y los íconos (vuelven a cargarse al entrar).
fn leave(app: &AppWindow, sub: &str) {
    if sub == "cleanup" {
        POLL_TIMER.with(Timer::stop);
        CLEANUP.set(None);
        return;
    }
    INSTALLED_GENERATION.fetch_add(1, Ordering::SeqCst);
    CATALOG_GENERATION.fetch_add(1, Ordering::SeqCst);
    INSTALLED.set(Vec::new());
    ICONS.with_borrow_mut(HashMap::clear);
    CATALOG_ICONS_REQUESTED.set(false);
    let state = app.global::<AppsState>();
    state.set_installed(ModelRc::default());
    for group in state.get_catalog().iter() {
        for (index, mut item) in group.apps.iter().enumerate() {
            if item.has_icon {
                item.icon = Image::default();
                item.has_icon = false;
                group.apps.set_row_data(index, item);
            }
        }
    }
}

fn load_installed(app: &AppWindow) {
    bridge::spawn(app, apps::installed, |app, list| {
        let jobs: Vec<(String, Source)> = list
            .iter()
            .filter(|item| !ICONS.with_borrow(|icons| icons.contains_key(&item.id)))
            .filter_map(|item| item.icon.clone().map(|path| (item.id.clone(), Source::FileIcon(path))))
            .collect();
        INSTALLED.set(list);
        refresh_visible(app);
        app.global::<AppsState>().set_loaded(true);
        images::load(app, jobs, ICON_SIZE, &INSTALLED_GENERATION, set_installed_icon);
    });
}

fn set_installed_icon(app: &AppWindow, id: String, pixels: images::Pixels) {
    let image = images::image(pixels);
    let model = app.global::<AppsState>().get_installed();
    if let Some((index, mut item)) = model.iter().enumerate().find(|(_, item)| item.id == id.as_str()) {
        item.icon = image.clone();
        item.has_icon = true;
        model.set_row_data(index, item);
    }
    ICONS.with_borrow_mut(|icons| icons.insert(id, image));
}

/// Íconos del catálogo: desde la caché y, si el usuario lo permite, desde el
/// sitio oficial de cada app (se descargan una sola vez).
fn load_catalog_icons(app: &AppWindow) {
    if CATALOG_ICONS_REQUESTED.replace(true) {
        return;
    }
    let Ok(groups) = apps::winget_catalog() else { return };
    let online = prefs::get().catalog_icons;
    let jobs: Vec<((usize, usize), Source)> = groups
        .iter()
        .enumerate()
        .flat_map(|(group_index, group)| {
            group.apps.iter().enumerate().filter_map(move |(index, item)| {
                let usable = !item.site.is_empty() && (online || icons::cached(&item.id).is_some());
                usable.then(|| {
                    ((group_index, index), Source::CatalogIcon { id: item.id.clone(), site: item.site.clone() })
                })
            })
        })
        .collect();
    images::load(app, jobs, ICON_SIZE, &CATALOG_GENERATION, set_catalog_icon);
}

fn set_catalog_icon(app: &AppWindow, (group, index): (usize, usize), pixels: images::Pixels) {
    let catalog = app.global::<AppsState>().get_catalog();
    let Some(group) = catalog.row_data(group) else { return };
    if let Some(mut item) = group.apps.row_data(index) {
        item.icon = images::image(pixels);
        item.has_icon = true;
        group.apps.set_row_data(index, item);
    }
}

fn load_winget(app: &AppWindow) {
    bridge::spawn(app, apps::winget_installed, |app, result| {
        match result {
            Ok(ids) => {
                let catalog = app.global::<AppsState>().get_catalog();
                for group in catalog.iter() {
                    for (index, mut item) in group.apps.iter().enumerate() {
                        item.installed = ids.iter().any(|id| id.eq_ignore_ascii_case(&item.id));
                        group.apps.set_row_data(index, item);
                    }
                }
            }
            Err(err) => toasts::error(&err),
        }
        app.global::<AppsState>().set_winget_loaded(true);
    });
}

/// Aplica búsqueda y orden a la lista de instaladas (actualizando fila por fila).
fn refresh_visible(app: &AppWindow) {
    let state = app.global::<AppsState>();
    let query = search::normalize(state.get_query().trim());
    let sort = state.get_sort();
    INSTALLED.with_borrow(|list| {
        let mut visible: Vec<&InstalledApp> = list
            .iter()
            .filter(|item| {
                query.is_empty() || search::normalize(&format!("{} {}", item.name, item.publisher)).contains(&query)
            })
            .collect();
        let by_name = |a: &&InstalledApp, b: &&InstalledApp| a.name.to_lowercase().cmp(&b.name.to_lowercase());
        match sort {
            1 => visible.sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| by_name(a, b))),
            2 => visible.sort_by(|a, b| b.install_date.cmp(&a.install_date).then_with(|| by_name(a, b))),
            _ => visible.sort_by(by_name),
        }
        let total_kb: u64 = visible.iter().map(|item| item.size_kb).sum();
        state.set_summary(if visible.len() == 1 { "1 app".into() } else { format!("{} apps", visible.len()).into() });
        state.set_total(
            if total_kb > 0 { format!("Espacio ocupado: {}", format::bytes(total_kb * 1024)) } else { String::new() }
                .into(),
        );
        let views: Vec<InstalledAppView> = visible.iter().map(|item| view(item)).collect();
        state.set_installed(models::sync(state.get_installed(), views));
    });
}

fn view(item: &InstalledApp) -> InstalledAppView {
    let size = if item.size_kb > 0 { format::bytes(item.size_kb * 1024) } else { String::new() };
    let detail = [item.publisher.as_str(), item.version.as_str(), size.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let icon = ICONS.with_borrow(|icons| icons.get(&item.id).cloned());
    InstalledAppView {
        id: item.id.as_str().into(),
        name: item.name.as_str().into(),
        detail: detail.into(),
        tint: format::tint_index(&item.name),
        letter: format::initial(&item.name).into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        can_uninstall: item.can_uninstall,
    }
}

/// Modifica una app del catálogo y devuelve lo que diga `change`.
fn update_catalog<R>(app: &AppWindow, id: &str, change: impl FnOnce(&mut CatalogAppView) -> R) -> Option<R> {
    let catalog = app.global::<AppsState>().get_catalog();
    for group in catalog.iter() {
        if let Some((index, mut item)) = group.apps.iter().enumerate().find(|(_, item)| item.id == id) {
            let result = change(&mut item);
            group.apps.set_row_data(index, item);
            return Some(result);
        }
    }
    None
}

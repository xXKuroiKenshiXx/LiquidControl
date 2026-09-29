//! Motor de ajustes (tweaks) en la interfaz: listas por categoría con el estado
//! real de cada ajuste, su ícono, su detalle técnico, cambios con respuesta
//! inmediata y aviso de reinicio. También declara los paneles Escritorio,
//! Privacidad y Juegos, que son solo listas del catálogo.

use liquidcontrol_core::tweaks::{self, Restart, Risk, Tweak, TweakState};
use liquidcontrol_core::util::clipboard;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::Pane;
use crate::toasts::{self, Kind};
use crate::{AppWindow, TweakCategory, TweakSection, TweakView, TweaksState, bridge, glyphs, models};

pub const DESKTOP: Pane = Pane::new("desktop", |app, _| load(app, Category::Desktop));
pub const PRIVACY: Pane = Pane::new("privacy", |app, _| load(app, Category::Privacy));
pub const GAMING: Pane = Pane::new("gaming", |app, _| load(app, Category::Gaming));

#[derive(Clone, Copy)]
pub enum Category {
    Privacy,
    Desktop,
    Optimize,
    Gaming,
    Accessibility,
    Updates,
}

impl Category {
    const ALL: [Category; 6] =
        [Self::Privacy, Self::Desktop, Self::Optimize, Self::Gaming, Self::Accessibility, Self::Updates];

    fn id(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::Desktop => "desktop",
            Self::Optimize => "optimize",
            Self::Gaming => "gaming",
            Self::Accessibility => "accessibility",
            Self::Updates => "updates",
        }
    }

    fn get(self, state: &TweaksState) -> TweakCategory {
        match self {
            Self::Privacy => state.get_privacy(),
            Self::Desktop => state.get_desktop(),
            Self::Optimize => state.get_optimize(),
            Self::Gaming => state.get_gaming(),
            Self::Accessibility => state.get_accessibility(),
            Self::Updates => state.get_updates(),
        }
    }

    fn set(self, state: &TweaksState, value: TweakCategory) {
        match self {
            Self::Privacy => state.set_privacy(value),
            Self::Desktop => state.set_desktop(value),
            Self::Optimize => state.set_optimize(value),
            Self::Gaming => state.set_gaming(value),
            Self::Accessibility => state.set_accessibility(value),
            Self::Updates => state.set_updates(value),
        }
    }
}

pub fn init(app: &AppWindow) {
    let state = app.global::<TweaksState>();

    let weak = app.as_weak();
    state.on_set(move |id, enabled| {
        let Some(app) = weak.upgrade() else { return };
        // Respuesta inmediata; el estado real llega cuando termina de aplicarse.
        // Mientras se aplica, otro clic sobre el mismo ajuste se ignora.
        let Some(previous) = update(&app, &id, |view| {
            if view.busy {
                return None;
            }
            let previous = view.state;
            view.state = i32::from(enabled);
            view.busy = true;
            Some(previous)
        })
        .flatten() else {
            return;
        };
        let id = id.to_string();
        let task_id = id.clone();
        bridge::spawn(
            &app,
            move || tweaks::set(&task_id, enabled),
            move |app, result| match result {
                Ok(tweak) => {
                    update(app, &id, |view| {
                        view.state = state_code(tweak.state);
                        view.busy = false;
                    });
                    raise_restart(app, tweak.restart);
                }
                Err(err) => {
                    update(app, &id, |view| {
                        view.state = previous;
                        view.busy = false;
                    });
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_copy_details(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(text) = update(&app, &id, |view| view.details.to_string()) else { return };
        bridge::spawn(
            &app,
            move || clipboard::set_text(&text),
            |_, result| match result {
                Ok(()) => toasts::show(Kind::Success, "Se copiaron los detalles del ajuste."),
                Err(err) => toasts::error(&err),
            },
        );
    });
}

/// Ajustes con un control dedicado en otro panel (ver ese panel para el
/// porqué): se excluyen acá para no mostrar el mismo interruptor dos veces.
const DEDICATED_ELSEWHERE: [&str; 1] = ["optimize.instant_menus"];

/// Carga (o recarga) los ajustes de una categoría con su estado real.
pub fn load(app: &AppWindow, category: Category) {
    bridge::spawn(
        app,
        move || tweaks::list(category.id()),
        move |app, result| match result {
            Ok(list) => {
                let list: Vec<_> = list.into_iter().filter(|t| !DEDICATED_ELSEWHERE.contains(&t.id.as_str())).collect();
                apply(&app.global::<TweaksState>(), category, build(&list));
            }
            Err(err) => toasts::error(&err),
        },
    );
}

/// Si la lista ya se muestra con las mismas secciones, se actualiza fila por fila
/// (las filas no se rearman); si cambió su forma, se reemplaza.
fn apply(state: &TweaksState, category: Category, new: TweakCategory) {
    let current = category.get(state);
    let same_shape = current.sections.row_count() == new.sections.row_count()
        && current
            .sections
            .iter()
            .zip(new.sections.iter())
            .all(|(old, new)| old.title == new.title && old.items.row_count() == new.items.row_count());
    if !same_shape {
        category.set(state, new);
        return;
    }
    for (old, new) in current.sections.iter().zip(new.sections.iter()) {
        models::sync(old.items.clone(), new.items.iter().collect());
    }
    if current.admin != new.admin {
        category.set(state, TweakCategory { sections: current.sections.clone(), admin: new.admin });
    }
}

/// Después de reiniciar el Explorador ya no hace falta avisar.
pub fn explorer_restarted(app: &AppWindow) {
    let state = app.global::<TweaksState>();
    if state.get_restart() == restart_code(Restart::Explorer) {
        state.set_restart(restart_code(Restart::None));
    }
}

fn build(list: &[Tweak]) -> TweakCategory {
    let mut sections: Vec<(String, Vec<TweakView>)> = Vec::new();
    for tweak in list {
        let view = TweakView {
            id: tweak.id.as_str().into(),
            title: tweak.title.as_str().into(),
            description: tweak.description.as_str().into(),
            risk: risk_code(tweak.risk),
            admin: tweak.admin,
            state: state_code(tweak.state),
            busy: false,
            details: tweak.details.join("\n").into(),
            icon: glyphs::for_tweak(tweak.icon.as_deref(), &tweak.title, &tweak.section, &tweak.category),
        };
        match sections.iter_mut().find(|(name, _)| *name == tweak.section) {
            Some((_, items)) => items.push(view),
            None => sections.push((tweak.section.clone(), vec![view])),
        }
    }
    let sections: Vec<TweakSection> = sections
        .into_iter()
        .map(|(title, items)| TweakSection { title: title.into(), items: ModelRc::new(VecModel::from(items)) })
        .collect();
    TweakCategory { sections: ModelRc::new(VecModel::from(sections)), admin: list.iter().any(|tweak| tweak.admin) }
}

/// Modifica un ajuste en la categoría donde esté y devuelve lo que diga `change`.
fn update<R>(app: &AppWindow, id: &str, change: impl FnOnce(&mut TweakView) -> R) -> Option<R> {
    let state = app.global::<TweaksState>();
    for category in Category::ALL {
        for section in category.get(&state).sections.iter() {
            if let Some((index, mut view)) = section.items.iter().enumerate().find(|(_, view)| view.id == id) {
                let result = change(&mut view);
                section.items.set_row_data(index, view);
                return Some(result);
            }
        }
    }
    None
}

fn raise_restart(app: &AppWindow, restart: Restart) {
    let state = app.global::<TweaksState>();
    let code = restart_code(restart);
    if code > state.get_restart() {
        state.set_restart(code);
    }
}

fn state_code(state: TweakState) -> i32 {
    match state {
        TweakState::Off => 0,
        TweakState::On => 1,
        TweakState::Partial => 2,
    }
}

fn risk_code(risk: Risk) -> i32 {
    match risk {
        Risk::Safe => 0,
        Risk::Moderate => 1,
        Risk::Advanced => 2,
    }
}

fn restart_code(restart: Restart) -> i32 {
    match restart {
        Restart::None => 0,
        Restart::Explorer => 1,
        Restart::Logoff => 2,
        Restart::Reboot => 3,
    }
}

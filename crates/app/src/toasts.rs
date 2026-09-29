//! Avisos flotantes (abajo, centrados) que se cierran solos.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use liquidcontrol_core::{AppError, actions, activity};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, VecModel};

use crate::{AppWindow, ToastView, Toasts, commands};

const VISIBLE_MAX: usize = 3;
const LIFETIME: Duration = Duration::from_secs(4);
const LIFETIME_WITH_ACTION: Duration = Duration::from_secs(8);

#[derive(Clone, Copy)]
pub enum Kind {
    Info = 0,
    Success = 1,
    Error = 2,
}

type Action = Box<dyn Fn(&AppWindow)>;

#[derive(Default)]
struct State {
    model: Rc<VecModel<ToastView>>,
    actions: Vec<(i32, Action)>,
    next_id: i32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub fn init(app: &AppWindow) {
    let toasts = app.global::<Toasts>();
    STATE.with_borrow(|state| toasts.set_items(ModelRc::from(state.model.clone())));
    let weak = app.as_weak();
    toasts.on_action(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let action = STATE.with_borrow_mut(|state| {
            let index = state.actions.iter().position(|(key, _)| *key == id)?;
            Some(state.actions.remove(index).1)
        });
        dismiss(id);
        if let Some(action) = action {
            action(&app);
        }
    });
}

pub fn show(kind: Kind, message: impl Into<SharedString>) {
    push(kind, message.into(), None);
}

pub fn show_with_action(
    kind: Kind,
    message: impl Into<SharedString>,
    label: impl Into<SharedString>,
    action: impl Fn(&AppWindow) + 'static,
) {
    push(kind, message.into(), Some((label.into(), Box::new(action))));
}

/// Muestra un error. Si faltan permisos, ofrece reiniciar la app como administrador.
pub fn error(err: &AppError) {
    if matches!(err, AppError::NeedsAdmin) && !actions::is_admin() {
        show_with_action(Kind::Error, err.to_string(), "Reiniciar como administrador", commands::relaunch_admin);
    } else {
        show(Kind::Error, err.to_string());
    }
}

fn push(kind: Kind, message: SharedString, action: Option<(SharedString, Action)>) {
    // Todo aviso queda también en la consola de actividad.
    match kind {
        Kind::Info => activity::info("app", message.as_str()),
        Kind::Success => activity::success("app", message.as_str()),
        Kind::Error => activity::error("app", message.as_str()),
    }
    let lifetime = if action.is_some() { LIFETIME_WITH_ACTION } else { LIFETIME };
    let id = STATE.with_borrow_mut(|state| {
        state.next_id += 1;
        let id = state.next_id;
        // Un mensaje repetido reemplaza al anterior en vez de apilarse.
        if let Some(index) = state.model.iter().position(|toast| toast.message == message) {
            let old = state.model.remove(index);
            state.actions.retain(|(key, _)| *key != old.id);
        }
        let label = match action {
            Some((label, action)) => {
                state.actions.push((id, action));
                label
            }
            None => SharedString::new(),
        };
        state.model.push(ToastView { id, message, kind: kind as i32, action: label });
        while state.model.row_count() > VISIBLE_MAX {
            let old = state.model.remove(0);
            state.actions.retain(|(key, _)| *key != old.id);
        }
        id
    });
    Timer::single_shot(lifetime, move || dismiss(id));
}

fn dismiss(id: i32) {
    STATE.with_borrow_mut(|state| {
        if let Some(index) = state.model.iter().position(|toast| toast.id == id) {
            state.model.remove(index);
        }
        state.actions.retain(|(key, _)| *key != id);
    });
}

//! Diálogo modal genérico (confirmaciones y pedido de contraseña). Cada diálogo
//! tiene una etiqueta y sus respuestas llegan a los manejadores registrados.

use std::cell::RefCell;
use std::collections::HashMap;

use slint::{ComponentHandle, SharedString};

use crate::{AppWindow, Dialog};

type ConfirmHandler = Box<dyn Fn(&AppWindow, SharedString)>;
type CancelHandler = Box<dyn Fn(&AppWindow)>;

thread_local! {
    static ON_CONFIRM: RefCell<HashMap<&'static str, ConfirmHandler>> = RefCell::default();
    static ON_CANCEL: RefCell<HashMap<&'static str, CancelHandler>> = RefCell::default();
}

pub fn init(app: &AppWindow) {
    let dialog = app.global::<Dialog>();
    let weak = app.as_weak();
    dialog.on_confirmed(move |tag, input| {
        let Some(app) = weak.upgrade() else { return };
        ON_CONFIRM.with_borrow(|handlers| match handlers.get(tag.as_str()) {
            Some(handler) => handler(&app, input),
            None => close(&app),
        });
    });
    let weak = app.as_weak();
    dialog.on_cancelled(move |tag| {
        let Some(app) = weak.upgrade() else { return };
        ON_CANCEL.with_borrow(|handlers| match handlers.get(tag.as_str()) {
            Some(handler) => handler(&app),
            None => close(&app),
        });
    });
}

/// Qué hacer cuando el usuario confirma el diálogo `tag`. Se registra al iniciar;
/// el manejador decide cuándo cerrar (ver [`close`] y [`set_busy`]).
pub fn register(tag: &'static str, handler: impl Fn(&AppWindow, SharedString) + 'static) {
    ON_CONFIRM.with_borrow_mut(|handlers| {
        handlers.insert(tag, Box::new(handler));
    });
}

/// Qué hacer si el usuario cancela `tag` (por defecto, solo cerrar).
pub fn register_cancel(tag: &'static str, handler: impl Fn(&AppWindow) + 'static) {
    ON_CANCEL.with_borrow_mut(|handlers| {
        handlers.insert(tag, Box::new(handler));
    });
}

/// Pregunta de confirmación (con botón destructivo si corresponde).
pub fn confirm(app: &AppWindow, tag: &'static str, title: &str, message: &str, confirm: &str, destructive: bool) {
    let dialog = app.global::<Dialog>();
    reset(&dialog, tag, title, message, confirm);
    dialog.set_destructive(destructive);
    dialog.set_open(true);
}

/// Pide un texto secreto (p. ej. la contraseña de una red Wi‑Fi).
pub fn ask_secret(app: &AppWindow, tag: &'static str, title: &str, message: &str, placeholder: &str, min_len: i32) {
    let dialog = app.global::<Dialog>();
    reset(&dialog, tag, title, message, "Conectar");
    dialog.set_has_input(true);
    dialog.set_secret(true);
    dialog.set_placeholder(placeholder.into());
    dialog.set_min_input(min_len);
    dialog.set_open(true);
}

/// Pide un texto visible (p. ej. el nombre de un plan de energía).
pub fn ask_text(
    app: &AppWindow,
    tag: &'static str,
    title: &str,
    message: &str,
    placeholder: &str,
    initial: &str,
    confirm: &str,
) {
    let dialog = app.global::<Dialog>();
    reset(&dialog, tag, title, message, confirm);
    dialog.set_has_input(true);
    dialog.set_secret(false);
    dialog.set_placeholder(placeholder.into());
    dialog.set_input(initial.into());
    dialog.set_min_input(1);
    dialog.set_open(true);
}

pub fn set_cancel_text(app: &AppWindow, text: &str) {
    app.global::<Dialog>().set_cancel_text(text.into());
}

/// Cambia el texto del diálogo abierto (p. ej. una cuenta regresiva).
pub fn set_message(app: &AppWindow, message: &str) {
    app.global::<Dialog>().set_message(message.into());
}

pub fn set_busy(app: &AppWindow, busy: bool) {
    let dialog = app.global::<Dialog>();
    dialog.set_busy(busy);
    if busy {
        dialog.set_error(SharedString::new());
    }
}

/// Muestra un error dentro del diálogo sin cerrarlo (p. ej. contraseña incorrecta).
pub fn set_error(app: &AppWindow, message: &str) {
    let dialog = app.global::<Dialog>();
    dialog.set_busy(false);
    dialog.set_error(message.into());
}

/// Cierra el diálogo y borra lo escrito (nunca queda una contraseña en la interfaz).
pub fn close(app: &AppWindow) {
    let dialog = app.global::<Dialog>();
    dialog.set_open(false);
    dialog.set_busy(false);
    dialog.set_input(SharedString::new());
}

fn reset(dialog: &Dialog, tag: &'static str, title: &str, message: &str, confirm: &str) {
    dialog.set_tag(tag.into());
    dialog.set_title(title.into());
    dialog.set_message(message.into());
    dialog.set_confirm_text(confirm.into());
    dialog.set_cancel_text("Cancelar".into());
    dialog.set_has_input(false);
    dialog.set_input(SharedString::new());
    dialog.set_placeholder(SharedString::new());
    dialog.set_min_input(0);
    dialog.set_destructive(false);
    dialog.set_busy(false);
    dialog.set_error(SharedString::new());
}

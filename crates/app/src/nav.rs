//! Navegación: historial Atrás/Adelante, título de la barra y resaltado de opciones.

use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, Model, SharedString, Timer, TimerMode};

use crate::{AppWindow, Nav, PaneInfo, Registry, panes};

const HIGHLIGHT_TIME: Duration = Duration::from_millis(2500);
const HISTORY_MAX: usize = 50;

#[derive(Clone, PartialEq, Eq)]
struct Location {
    pane: String,
    sub: String,
}

#[derive(Default)]
struct History {
    entries: Vec<Location>,
    index: usize,
}

thread_local! {
    static HISTORY: RefCell<History> = RefCell::default();
    static HIGHLIGHT_TIMER: Timer = Timer::default();
}

/// Conecta la navegación y muestra el panel inicial (`--pane` o General).
pub fn init(app: &AppWindow, pane: &str, sub: &str) {
    let nav = app.global::<Nav>();
    let weak = app.as_weak();
    nav.on_open(move |pane, sub, item| {
        if let Some(app) = weak.upgrade() {
            open(&app, &pane, &sub, &item);
        }
    });
    let weak = app.as_weak();
    nav.on_back(move || {
        if let Some(app) = weak.upgrade() {
            step(&app, -1);
        }
    });
    let weak = app.as_weak();
    nav.on_forward(move || {
        if let Some(app) = weak.upgrade() {
            step(&app, 1);
        }
    });
    let (pane, sub) = if pane_info(app, pane).is_some() { (pane, sub) } else { ("general", "") };
    open(app, pane, sub, "");
}

/// Abre un panel (y opcionalmente una subpágina u opción a resaltar).
pub fn open(app: &AppWindow, pane: &str, sub: &str, item: &str) {
    let (sub, highlight) =
        if sub.is_empty() && !item.is_empty() { panes::target(pane, item) } else { (sub.to_owned(), item.to_owned()) };
    let location = Location { pane: pane.to_owned(), sub };
    HISTORY.with_borrow_mut(|history| {
        if history.entries.get(history.index) != Some(&location) {
            history.entries.truncate(history.index + 1);
            history.entries.push(location.clone());
            if history.entries.len() > HISTORY_MAX {
                history.entries.remove(0);
            }
            history.index = history.entries.len() - 1;
        }
    });
    // Volver a abrir el panel actual también sirve para refrescar sus datos.
    show(app, &location);
    set_highlight(app, &highlight);
}

fn step(app: &AppWindow, delta: isize) {
    let location = HISTORY.with_borrow_mut(|history| {
        let target = history.index.checked_add_signed(delta).filter(|i| *i < history.entries.len())?;
        history.index = target;
        Some(history.entries[target].clone())
    });
    if let Some(location) = location {
        show(app, &location);
        set_highlight(app, "");
    }
}

fn show(app: &AppWindow, location: &Location) {
    let nav = app.global::<Nav>();
    let title = panes::sub_title(&location.pane, &location.sub)
        .map(SharedString::from)
        .or_else(|| pane_info(app, &location.pane).map(|pane| pane.title))
        .unwrap_or_default();
    // El panel (o la subpágina) que se deja puede liberar memoria.
    let (previous_pane, previous_sub) = (nav.get_pane(), nav.get_sub());
    if previous_pane.as_str() != location.pane || previous_sub.as_str() != location.sub {
        panes::leave(app, &previous_pane, &previous_sub);
    }
    nav.set_pane(location.pane.as_str().into());
    nav.set_sub(location.sub.as_str().into());
    nav.set_title(title);
    HISTORY.with_borrow(|history| {
        nav.set_can_back(history.index > 0);
        nav.set_can_forward(history.index + 1 < history.entries.len());
    });
    panes::enter(app, &location.pane, &location.sub);
}

fn set_highlight(app: &AppWindow, item: &str) {
    app.global::<Nav>().set_highlight(item.into());
    if item.is_empty() {
        return;
    }
    let weak = app.as_weak();
    HIGHLIGHT_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, HIGHLIGHT_TIME, move || {
            if let Some(app) = weak.upgrade() {
                app.global::<Nav>().set_highlight(SharedString::new());
            }
        });
    });
}

/// Panel activo (para relanzar la app en el mismo lugar).
pub fn current_pane(app: &AppWindow) -> String {
    app.global::<Nav>().get_pane().into()
}

/// Datos de un panel de la barra lateral (título, ícono y color).
pub fn pane_info(app: &AppWindow, id: &str) -> Option<PaneInfo> {
    let registry = app.global::<Registry>();
    let account = registry.get_account();
    if account.id == id {
        return Some(account);
    }
    [
        registry.get_group1(),
        registry.get_group2(),
        registry.get_group3(),
        registry.get_group4(),
        registry.get_group5(),
        registry.get_group6(),
        registry.get_group7(),
    ]
    .into_iter()
    .find_map(|group| group.iter().find(|pane| pane.id == id))
}

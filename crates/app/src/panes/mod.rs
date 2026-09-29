//! Controladores de los paneles. Cada módulo declara su `PANE`: qué hacer al
//! iniciar la app, al entrar y al salir del panel, los títulos de sus subpáginas
//! y adónde lleva cada resultado de búsqueda. Este registro los reúne: agregar un
//! panel nuevo es sumar una línea a `PANES`.

use crate::AppWindow;

mod accessibility;
mod account;
mod airdrop;
mod app_settings;
mod appearance;
mod apps;
mod bluetooth;
mod displays;
mod energy;
mod general;
mod icloud;
mod input;
mod languages;
mod mixer;
mod network;
mod notifications;
mod optimize;
mod security;
mod sound;
mod tweaks;
mod wallpaper;
mod wifi;

/// Un panel de la barra lateral (el id coincide con el de `Registry` en `globals.slint`).
#[derive(Clone, Copy)]
pub struct Pane {
    pub id: &'static str,
    /// Conecta los callbacks del panel (una sola vez, al iniciar).
    pub init: fn(&AppWindow),
    /// Carga los datos al mostrar el panel o una de sus subpáginas.
    pub enter: fn(&AppWindow, &str),
    /// Al dejar el panel o una subpágina: libera lo que ocupa memoria.
    pub leave: fn(&AppWindow, &str),
    /// Título de la barra para una subpágina.
    pub sub_title: fn(&str) -> Option<String>,
    /// Destino de un resultado de búsqueda: `(subpágina, opción a resaltar)`.
    pub target: fn(&str) -> (String, String),
}

fn nothing(_: &AppWindow) {}

fn nothing_on_leave(_: &AppWindow, _: &str) {}

fn no_title(_: &str) -> Option<String> {
    None
}

fn highlight(item: &str) -> (String, String) {
    (String::new(), item.to_owned())
}

impl Pane {
    pub const fn new(id: &'static str, enter: fn(&AppWindow, &str)) -> Self {
        Self { id, init: nothing, enter, leave: nothing_on_leave, sub_title: no_title, target: highlight }
    }

    pub const fn with_init(self, init: fn(&AppWindow)) -> Self {
        Self { init, ..self }
    }

    pub const fn with_leave(self, leave: fn(&AppWindow, &str)) -> Self {
        Self { leave, ..self }
    }

    pub const fn with_sub_title(self, sub_title: fn(&str) -> Option<String>) -> Self {
        Self { sub_title, ..self }
    }

    pub const fn with_subpages(
        self,
        sub_title: fn(&str) -> Option<String>,
        target: fn(&str) -> (String, String),
    ) -> Self {
        Self { sub_title, target, ..self }
    }
}

/// Título de una subpágina a partir de una lista fija `(id, título)`.
pub fn fixed_title(list: &[(&str, &str)], sub: &str) -> Option<String> {
    list.iter().find(|(id, _)| *id == sub).map(|(_, title)| (*title).to_owned())
}

/// Si lo buscado es una subpágina se abre; si no, se resalta en la página principal.
pub fn fixed_target(list: &[(&str, &str)], item: &str) -> (String, String) {
    if list.iter().any(|(id, _)| *id == item) { (item.to_owned(), String::new()) } else { highlight(item) }
}

const PANES: &[Pane] = &[
    account::PANE,
    wifi::PANE,
    airdrop::PANE,
    bluetooth::PANE,
    network::PANE,
    notifications::PANE,
    sound::PANE,
    general::PANE,
    appearance::PANE,
    wallpaper::PANE,
    accessibility::PANE,
    tweaks::DESKTOP,
    displays::PANE,
    energy::PANE,
    security::PANE,
    tweaks::PRIVACY,
    optimize::PANE,
    tweaks::GAMING,
    apps::PANE,
    input::MOUSE,
    input::KEYBOARD,
    app_settings::PANE,
];

/// Partes compartidas o que viven dentro de otro panel (iCloud en Cuenta,
/// mezclador en Sonido).
const SHARED: &[fn(&AppWindow)] = &[tweaks::init, icloud::init, mixer::init];

fn find(id: &str) -> Option<&'static Pane> {
    PANES.iter().find(|pane| pane.id == id)
}

pub fn init(app: &AppWindow) {
    for init in SHARED {
        init(app);
    }
    for pane in PANES {
        (pane.init)(app);
    }
}

pub fn enter(app: &AppWindow, pane: &str, sub: &str) {
    if let Some(pane) = find(pane) {
        (pane.enter)(app, sub);
    }
}

pub fn leave(app: &AppWindow, pane: &str, sub: &str) {
    if let Some(pane) = find(pane) {
        (pane.leave)(app, sub);
    }
}

pub fn sub_title(pane: &str, sub: &str) -> Option<String> {
    if sub.is_empty() { None } else { find(pane).and_then(|pane| (pane.sub_title)(sub)) }
}

pub fn target(pane: &str, item: &str) -> (String, String) {
    find(pane).map_or_else(|| highlight(item), |pane| (pane.target)(item))
}

/// El Explorador se reinició: los avisos de "reiniciar el Explorador" ya no hacen falta.
pub fn explorer_restarted(app: &AppWindow) {
    tweaks::explorer_restarted(app);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{PANES, fixed_target, fixed_title};

    #[test]
    fn pane_ids_are_unique() {
        let mut seen = HashSet::new();
        assert!(PANES.iter().all(|pane| seen.insert(pane.id)));
    }

    #[test]
    fn fixed_subpages() {
        let list = [("about", "Información")];
        assert_eq!(fixed_title(&list, "about").as_deref(), Some("Información"));
        assert_eq!(fixed_target(&list, "about"), ("about".to_owned(), String::new()));
        assert_eq!(fixed_target(&list, "storage"), (String::new(), "storage".to_owned()));
    }
}

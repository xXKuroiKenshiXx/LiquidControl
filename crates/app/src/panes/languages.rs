//! Idiomas de escritura y distribuciones de teclado (parte del panel Teclado):
//! teclado predeterminado, agregar y quitar idiomas y distribuciones.

use std::cell::RefCell;

use liquidcontrol_core::AppResult;
use liquidcontrol_core::keyboard::{self, LANGUAGES, LAYOUTS, Languages};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::toasts::{self, Kind};
use crate::{AppWindow, KeyboardState, LanguageView, LayoutView, bridge, models};

/// Lo que representa cada opción de los menús.
#[derive(Default)]
struct Choices {
    /// Métodos de entrada en el orden del menú "Teclado predeterminado".
    tips: Vec<String>,
    /// Idiomas del menú "Agregar un idioma" (el índice 0 del menú es "Agregar…").
    languages: Vec<&'static str>,
    /// Idioma principal: ahí se agregan las distribuciones nuevas.
    main: String,
}

thread_local! {
    static CHOICES: RefCell<Choices> = RefCell::default();
}

pub fn init(app: &AppWindow) {
    let state = app.global::<KeyboardState>();
    let layout_options: Vec<SharedString> =
        std::iter::once("Agregar…").chain(LAYOUTS.iter().map(|(_, name)| *name)).map(SharedString::from).collect();
    state.set_layout_options(ModelRc::new(VecModel::from(layout_options)));

    let weak = app.as_weak();
    state.on_set_default_layout(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(tip) =
            CHOICES.with_borrow(|choices| usize::try_from(index).ok().and_then(|i| choices.tips.get(i).cloned()))
        else {
            return;
        };
        app.global::<KeyboardState>().set_default_index(index);
        apply(&app, move || keyboard::set_default_layout(&tip), "Se cambió el teclado predeterminado.");
    });

    let weak = app.as_weak();
    state.on_add_language(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(tag) = CHOICES
            .with_borrow(|choices| usize::try_from(index - 1).ok().and_then(|i| choices.languages.get(i).copied()))
        else {
            return;
        };
        apply(&app, move || keyboard::add_language(tag), "Se agregó el idioma.");
    });

    let weak = app.as_weak();
    state.on_remove_language(move |tag| {
        let Some(app) = weak.upgrade() else { return };
        let tag = tag.to_string();
        apply(&app, move || keyboard::remove_language(&tag), "Se quitó el idioma.");
    });

    let weak = app.as_weak();
    state.on_add_layout(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&(klid, _)) = usize::try_from(index - 1).ok().and_then(|i| LAYOUTS.get(i)) else { return };
        let main = CHOICES.with_borrow(|choices| choices.main.clone());
        if main.is_empty() {
            return;
        }
        apply(&app, move || keyboard::add_layout(&main, klid), "Se agregó la distribución de teclado.");
    });

    let weak = app.as_weak();
    state.on_remove_layout(move |tag, tip| {
        let Some(app) = weak.upgrade() else { return };
        let (tag, tip) = (tag.to_string(), tip.to_string());
        apply(&app, move || keyboard::remove_layout(&tag, &tip), "Se quitó la distribución de teclado.");
    });
}

pub fn load(app: &AppWindow) {
    bridge::spawn(app, keyboard::list, |app, result| match result {
        Ok(languages) => show(app, &languages),
        Err(err) => toasts::error(&err),
    });
}

fn show(app: &AppWindow, languages: &Languages) {
    let state = app.global::<KeyboardState>();
    let several = languages.list.len() > 1;
    let language_views: Vec<LanguageView> = languages
        .list
        .iter()
        .map(|language| LanguageView {
            tag: language.tag.as_str().into(),
            name: language.name.as_str().into(),
            layouts: language.layouts.iter().map(|layout| layout.name.as_str()).collect::<Vec<_>>().join(", ").into(),
            can_remove: several,
        })
        .collect();
    let mut layout_views = Vec::new();
    let mut tips = Vec::new();
    let mut default_options: Vec<SharedString> = Vec::new();
    for language in &languages.list {
        for layout in &language.layouts {
            layout_views.push(LayoutView {
                tag: language.tag.as_str().into(),
                tip: layout.tip.as_str().into(),
                name: layout.name.as_str().into(),
                language: language.name.as_str().into(),
                can_remove: language.layouts.len() > 1,
            });
            tips.push(layout.tip.clone());
            default_options.push(format!("{} · {}", layout.name, language.name).into());
        }
    }
    // Sin elección explícita, Windows usa el primer teclado del idioma principal.
    let default_index = languages
        .default_tip
        .as_deref()
        .and_then(|tip| tips.iter().position(|known| known.eq_ignore_ascii_case(tip)))
        .unwrap_or(0);
    let offered: Vec<(&'static str, &'static str)> = LANGUAGES
        .iter()
        .copied()
        .filter(|(tag, _)| !languages.list.iter().any(|language| language.tag.eq_ignore_ascii_case(tag)))
        .collect();
    let language_options: Vec<SharedString> = std::iter::once(SharedString::from("Agregar…"))
        .chain(offered.iter().map(|(_, name)| SharedString::from(*name)))
        .collect();

    state.set_languages(models::sync(state.get_languages(), language_views));
    state.set_layouts(models::sync(state.get_layouts(), layout_views));
    state.set_default_options(ModelRc::new(VecModel::from(default_options)));
    state.set_default_index(default_index as i32);
    state.set_language_options(ModelRc::new(VecModel::from(language_options)));
    CHOICES.set(Choices {
        tips,
        languages: offered.iter().map(|(tag, _)| *tag).collect(),
        main: languages.list.first().map(|language| language.tag.clone()).unwrap_or_default(),
    });
    state.set_languages_loaded(true);
}

/// Aplica un cambio (uno por vez) y vuelve a leer la lista real.
fn apply(app: &AppWindow, work: impl FnOnce() -> AppResult<()> + Send + 'static, done: &'static str) {
    let state = app.global::<KeyboardState>();
    if state.get_languages_busy() {
        return;
    }
    state.set_languages_busy(true);
    bridge::spawn(app, work, move |app, result| {
        app.global::<KeyboardState>().set_languages_busy(false);
        match result {
            Ok(()) => toasts::show(Kind::Success, done),
            Err(err) => toasts::error(&err),
        }
        load(app);
    });
}

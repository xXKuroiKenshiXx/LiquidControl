//! Fondo de pantalla (panel propio, debajo de Apariencia): vista previa,
//! galería con miniaturas en segundo plano, ajuste, colores sólidos, fondos
//! gratis en línea y selector de archivos.
//! Cada cambio se puede deshacer: se recuerda el fondo anterior.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use liquidcontrol_core::wallpaper::{self, Position, WallpaperImage};
use liquidcontrol_core::wallpaper_search::{self, OnlineWallpaper};
use liquidcontrol_core::{AppResult, media, picker};
use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::Pane;
use crate::images::{self, Source};
use crate::toasts::{self, Kind};
use crate::{AppWindow, Nav, WallpaperCollection, WallpaperState, WallpaperThumb, bridge, chrome};

/// Prefijo del `path`/`key` de un resultado de búsqueda en línea: no es una
/// ruta real todavía (la imagen completa se descarga recién al elegirla).
const ONLINE_PREFIX: &str = "online:";

pub const PANE: Pane = Pane::new("wallpaper", |app, _| enter(app)).with_init(init).with_leave(|app, _| leave(app));

const THUMB_SIZE: u32 = 160;
const PREVIEW_SIZE: u32 = 640;
/// Negro, grafito, plata, azul marino, índigo, azul, celeste, verde, amarillo,
/// naranja, rojo y violeta: una fila completa, como la paleta de macOS.
const COLORS: [u32; 12] = [
    0x1d1d1f, 0x636366, 0xaeaeb2, 0x1c3d6e, 0x5e5ce6, 0x0a84ff, 0x64d2ff, 0x30d158, 0xffd60a, 0xff9f0a, 0xff375f,
    0xbf5af2,
];

/// Cambia cada vez que se recarga o se abandona la galería: las miniaturas de
/// una carga anterior se descartan en lugar de ocupar memoria.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Lo mismo, para los resultados de la búsqueda en línea (lista aparte).
static SEARCH_GENERATION: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Fondo que había antes del último cambio (para "Deshacer").
    static PREVIOUS: RefCell<Option<wallpaper::WallpaperState>> = const { RefCell::new(None) };
    /// Últimos resultados de búsqueda en línea (para poder descargar la
    /// imagen completa cuando el usuario elige una miniatura).
    static ONLINE_RESULTS: RefCell<Vec<OnlineWallpaper>> = RefCell::default();
}

fn init(app: &AppWindow) {
    let state = app.global::<WallpaperState>();
    let labels: Vec<SharedString> = Position::ALL.iter().map(|position| position.label().into()).collect();
    state.set_position_labels(ModelRc::new(VecModel::from(labels)));
    let colors: Vec<Color> = COLORS.iter().map(|rgb| color(*rgb)).collect();
    state.set_colors(ModelRc::new(VecModel::from(colors)));

    let weak = app.as_weak();
    state.on_choose(move |path| {
        let Some(app) = weak.upgrade() else { return };
        if let Some(id) = path.strip_prefix(ONLINE_PREFIX) {
            apply_online(&app, id);
            return;
        }
        let path = PathBuf::from(path.as_str());
        change(&app, move || wallpaper::set(&path));
    });

    let weak = app.as_weak();
    state.on_search_wallpapers(move |query| {
        let Some(app) = weak.upgrade() else { return };
        search(&app, query.to_string());
    });

    let weak = app.as_weak();
    state.on_set_position(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&position) = usize::try_from(index).ok().and_then(|i| Position::ALL.get(i)) else { return };
        app.global::<WallpaperState>().set_position(index);
        bridge::spawn(
            &app,
            move || wallpaper::set_position(position),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load_current(app, true);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_color(move |tone| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<WallpaperState>();
        state.set_color(tone);
        state.set_current_key(SharedString::new());
        state.set_current_name("Color sólido".into());
        state.set_has_preview(false);
        state.set_preview(Image::default());
        let rgb = (tone.red(), tone.green(), tone.blue());
        change(&app, move || wallpaper::set_color(rgb));
    });

    let weak = app.as_weak();
    state.on_pick_file(move || {
        let Some(app) = weak.upgrade() else { return };
        let owner = chrome::hwnd(&app);
        bridge::spawn(
            &app,
            move || picker::pick_image(owner),
            |app, result| match result {
                Ok(Some(path)) => change(app, move || wallpaper::set(&path)),
                Ok(None) => {}
                Err(err) => toasts::error(&err),
            },
        );
    });

    let weak = app.as_weak();
    state.on_restore_previous(move || {
        if let Some(app) = weak.upgrade() {
            restore_previous(&app);
        }
    });
}

/// Al entrar al panel: fondo actual con vista previa y galería.
fn enter(app: &AppWindow) {
    load_current(app, true);
    load_gallery(app);
}

/// Al salir se sueltan las miniaturas (la app apunta a menos de 100 MB).
fn leave(app: &AppWindow) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    SEARCH_GENERATION.fetch_add(1, Ordering::SeqCst);
    let state = app.global::<WallpaperState>();
    state.set_collections(ModelRc::default());
    state.set_search_results(ModelRc::default());
    state.set_preview(Image::default());
    state.set_has_preview(false);
    ONLINE_RESULTS.set(Vec::new());
}

/// Busca fondos gratis en Wallhaven (hechos por la comunidad) y arma la
/// grilla de miniaturas (se descargan en segundo plano, igual que la galería
/// local).
fn search(app: &AppWindow, query: String) {
    let state = app.global::<WallpaperState>();
    state.set_searching(true);
    state.set_search_results(ModelRc::default());
    bridge::spawn(
        app,
        move || wallpaper_search::search(&query),
        |app, result| {
            let state = app.global::<WallpaperState>();
            state.set_searching(false);
            match result {
                Ok(list) => {
                    let thumbs: Vec<WallpaperThumb> = list
                        .iter()
                        .map(|item| WallpaperThumb {
                            path: format!("{ONLINE_PREFIX}{}", item.id).into(),
                            key: format!("{ONLINE_PREFIX}{}", item.id).into(),
                            name: item.resolution.as_str().into(),
                            image: Image::default(),
                            loaded: false,
                        })
                        .collect();
                    let jobs: Vec<(usize, Source)> =
                        list.iter().cloned().enumerate().map(|(i, item)| (i, Source::OnlineWallpaper(item))).collect();
                    state.set_search_results(ModelRc::new(VecModel::from(thumbs)));
                    ONLINE_RESULTS.set(list);
                    images::load(app, jobs, THUMB_SIZE, &SEARCH_GENERATION, set_search_thumb);
                }
                Err(err) => toasts::error(&err),
            }
        },
    );
}

fn set_search_thumb(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let results = app.global::<WallpaperState>().get_search_results();
    if let Some(mut thumb) = results.row_data(index) {
        thumb.image = images::image(pixels);
        thumb.loaded = true;
        results.set_row_data(index, thumb);
    }
}

/// Descarga la imagen completa de un resultado elegido y la aplica (misma
/// lógica que un archivo elegido a mano, una vez que ya está en disco).
fn apply_online(app: &AppWindow, id: &str) {
    let state = app.global::<WallpaperState>();
    if state.get_downloading() {
        return;
    }
    let Some(item) = ONLINE_RESULTS.with_borrow(|list| list.iter().find(|item| item.id == id).cloned()) else {
        return;
    };
    state.set_downloading(true);
    bridge::spawn(
        app,
        move || wallpaper_search::download_full(&item),
        |app, result| {
            app.global::<WallpaperState>().set_downloading(false);
            match result {
                Ok(path) => change(app, move || wallpaper::set(&path)),
                Err(err) => toasts::error(&err),
            }
        },
    );
}

/// Aplica un cambio de fondo recordando el anterior y ofrece deshacerlo.
fn change(app: &AppWindow, apply: impl FnOnce() -> AppResult<()> + Send + 'static) {
    bridge::spawn(
        app,
        move || {
            let before = wallpaper::current().ok();
            apply().map(|()| before)
        },
        |app, result| {
            match result {
                Ok(before) => {
                    if before.is_some() {
                        PREVIOUS.set(before);
                    }
                    toasts::show_with_action(
                        Kind::Success,
                        "Se cambió el fondo de pantalla.",
                        "Deshacer",
                        restore_previous,
                    );
                }
                Err(err) => toasts::error(&err),
            }
            load_current(app, true);
        },
    );
}

/// Vuelve al fondo anterior (y lo reemplazado pasa a ser el nuevo "anterior").
fn restore_previous(app: &AppWindow) {
    let Some(previous) = PREVIOUS.take() else { return };
    change(app, move || apply_state(&previous));
}

fn apply_state(state: &wallpaper::WallpaperState) -> AppResult<()> {
    match &state.path {
        Some(path) => {
            wallpaper::set(path)?;
            wallpaper::set_position(state.position)
        }
        None => wallpaper::set_color(state.color),
    }
}

fn load_current(app: &AppWindow, with_preview: bool) {
    bridge::spawn(
        app,
        move || {
            let current = wallpaper::current()?;
            let preview = current
                .path
                .as_deref()
                .filter(|_| with_preview)
                .and_then(|path| media::thumbnail(path, PREVIEW_SIZE).ok())
                .map(|thumb| images::pixels(&thumb));
            Ok((current, preview))
        },
        |app, result: AppResult<_>| match result {
            Ok((current, preview)) => show_current(app, &current, preview),
            Err(err) => toasts::error(&err),
        },
    );
}

fn show_current(app: &AppWindow, current: &wallpaper::WallpaperState, preview: Option<images::Pixels>) {
    let state = app.global::<WallpaperState>();
    let (r, g, b) = current.color;
    state.set_color(Color::from_rgb_u8(r, g, b));
    state.set_current_key(current.path.as_deref().map(key).unwrap_or_default().into());
    state.set_current_name(display_name(current).into());
    state.set_position(Position::ALL.iter().position(|p| *p == current.position).unwrap_or(0) as i32);
    match preview {
        Some(pixels) => {
            state.set_preview(images::image(pixels));
            state.set_has_preview(true);
        }
        None => state.set_has_preview(false),
    }
    let previous = PREVIOUS.with_borrow(|previous| previous.as_ref().map(display_name)).unwrap_or_default();
    state.set_previous_name(previous.into());
    state.set_loaded(true);
}

fn display_name(state: &wallpaper::WallpaperState) -> String {
    state
        .path
        .as_deref()
        .and_then(Path::file_stem)
        .map_or_else(|| "Color sólido".to_owned(), |stem| stem.to_string_lossy().into_owned())
}

fn load_gallery(app: &AppWindow) {
    bridge::spawn(app, wallpaper::gallery, |app, gallery| {
        // Si el usuario ya se fue de la subpágina, no se carga nada.
        if app.global::<Nav>().get_pane() != "wallpaper" {
            return;
        }
        let mut groups: Vec<(String, Vec<&WallpaperImage>)> = Vec::new();
        for image in &gallery {
            match groups.iter_mut().find(|(title, _)| *title == image.collection) {
                Some((_, list)) => list.push(image),
                None => groups.push((image.collection.clone(), vec![image])),
            }
        }
        let mut jobs = Vec::new();
        let collections: Vec<WallpaperCollection> = groups
            .iter()
            .enumerate()
            .map(|(group, (title, list))| {
                let thumbs: Vec<WallpaperThumb> = list
                    .iter()
                    .enumerate()
                    .map(|(index, image)| {
                        jobs.push(((group, index), Source::Thumbnail(image.path.clone())));
                        WallpaperThumb {
                            path: image.path.to_string_lossy().as_ref().into(),
                            key: key(&image.path).into(),
                            name: image.name.as_str().into(),
                            image: Image::default(),
                            loaded: false,
                        }
                    })
                    .collect();
                WallpaperCollection { title: title.as_str().into(), images: ModelRc::new(VecModel::from(thumbs)) }
            })
            .collect();
        app.global::<WallpaperState>().set_collections(ModelRc::new(VecModel::from(collections)));
        images::load(app, jobs, THUMB_SIZE, &GENERATION, set_thumb);
    });
}

fn set_thumb(app: &AppWindow, (group, index): (usize, usize), pixels: images::Pixels) {
    let collections = app.global::<WallpaperState>().get_collections();
    let Some(collection) = collections.row_data(group) else { return };
    if let Some(mut thumb) = collection.images.row_data(index) {
        thumb.image = images::image(pixels);
        thumb.loaded = true;
        collection.images.set_row_data(index, thumb);
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn color(rgb: u32) -> Color {
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

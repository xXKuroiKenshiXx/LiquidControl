//! iCloud (subpágina de Cuenta): estado de iCloud para Windows, carpetas
//! sincronizadas y fotos recientes con miniatura.
//!
//! Las fotos que están solo en la nube muestran la miniatura que Windows ya
//! tenga guardada; generarla desde cero obliga a iCloud a descargar la foto, así
//! que eso solo pasa si el usuario lo permite (queda guardado en las preferencias).
//! Nunca se manejan credenciales de Apple: solo se abren la app, la Store o la web.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use liquidcontrol_core::icloud::{self, CloudPhoto, Target};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::images::{self, Source};
use crate::photo_search::{self, Traits};
use crate::{AppWindow, ICloudState, Nav, PhotoView, bridge, prefs, toasts};

const PHOTO_SIZE: u32 = 200;
/// Cuántas fotos entran en el buscador (de la más nueva a la más vieja).
const SEARCHABLE: usize = 400;
/// Cuántos resultados se muestran.
const MAX_FOUND: usize = 24;
/// Cada cuántas fotos analizadas se refresca la lista de resultados.
const BATCH: usize = 24;
/// Tope de la página "Ver todas": con miles de fotos, mostrarlas todas a la
/// vez tendría cada una con su miniatura decodificada en memoria a la vez
/// (la app apunta a menos de 100 MB en total), así que se muestran las más
/// nuevas hasta este número y se avisa cuántas quedaron afuera.
const ALL_PHOTOS_LIMIT: usize = 200;
/// Más chica que `PHOTO_SIZE`: son muchas más fotos a la vez en esta página.
const ALL_PHOTOS_THUMB_SIZE: u32 = 96;
/// Tope de fotos que están solo en la nube que se miran en una búsqueda:
/// mirarlas obliga a iCloud a descargarlas, así que ni con permiso se bajan
/// cientos de una.
const CLOUD_LOOK_LIMIT: usize = 80;

/// Descarta miniaturas de una carga anterior o de una página que ya se cerró.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Lo mismo para las miniaturas de los resultados de la búsqueda.
static FOUND_GENERATION: AtomicU32 = AtomicU32::new(0);
/// Cambia al escribir otra cosa o al salir: el análisis en curso se abandona.
static SEARCH_GENERATION: AtomicU32 = AtomicU32::new(0);
/// Lo mismo para la página "Ver todas".
static ALL_GENERATION: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Fotos de la carpeta de iCloud para buscar (nombre, fecha y si está en la nube).
    static PHOTOS: RefCell<Vec<CloudPhoto>> = RefCell::default();
    /// Cómo se ve cada foto ya mirada, para no volver a mirarla.
    static LOOKS: RefCell<photo_search::Cache> = RefCell::default();
    /// Resultados que se están mostrando (para abrir el que se toque).
    static FOUND: RefCell<Vec<CloudPhoto>> = RefCell::default();
}

pub fn init(app: &AppWindow) {
    let state = app.global::<ICloudState>();

    let weak = app.as_weak();
    state.on_open(move |target| {
        let Some(app) = weak.upgrade() else { return };
        let target = match target.as_str() {
            "drive" => Target::Drive,
            "photos" => Target::Photos,
            "app" => Target::App,
            "store" => Target::Store,
            "web" => Target::Web,
            "passwords" => Target::Passwords,
            _ => return,
        };
        bridge::spawn(
            &app,
            move || icloud::open(target),
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_search_photos(move |query| {
        let Some(app) = weak.upgrade() else { return };
        search(&app, query.to_string());
    });

    let weak = app.as_weak();
    state.on_open_photo(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let path = PathBuf::from(path.as_str());
        bridge::spawn(
            &app,
            move || icloud::open_photo(&path),
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_download_photo(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let path = PathBuf::from(path.as_str());
        toasts::show(toasts::Kind::Info, "Descargando…");
        bridge::spawn(
            &app,
            move || icloud::download_photo(&path),
            |app, result| match result {
                Ok(()) => {
                    toasts::show(toasts::Kind::Success, "Lista: ya está guardada en tu PC.");
                    refresh_current(app);
                }
                Err(err) => toasts::error(&err),
            },
        );
    });

    let weak = app.as_weak();
    state.on_delete_photo(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let path = PathBuf::from(path.as_str());
        bridge::spawn(
            &app,
            move || icloud::delete_photo(&path),
            |app, result| match result {
                Ok(()) => {
                    toasts::show(toasts::Kind::Success, "Se mandó a la Papelera.");
                    refresh_current(app);
                }
                Err(err) => toasts::error(&err),
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_download_thumbnails(move |on| {
        let Some(app) = weak.upgrade() else { return };
        if let Err(err) = prefs::update(&app, |prefs| prefs.icloud_thumbnails = on) {
            toasts::error(&err);
        }
        log::info!(
            target: "icloud",
            "Miniaturas de fotos solo en la nube: {}",
            if on { "permitido descargar" } else { "solo las ya guardadas" }
        );
        enter(&app);
    });
}

pub fn enter(app: &AppWindow) {
    let download = prefs::get().icloud_thumbnails;
    app.global::<ICloudState>().set_download_thumbnails(download);
    bridge::spawn(app, icloud::status, move |app, status| {
        let state = app.global::<ICloudState>();
        state.set_installed(status.installed);
        state.set_drive_folder(
            status.drive_folder.as_deref().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default().into(),
        );
        state.set_has_photos_folder(status.photos_folder.is_some());
        let photos = status.recent_photos;
        state.set_cloud_only_count(photos.iter().filter(|photo| photo.cloud_only).count() as i32);
        state.set_has_heic(photos.iter().any(|photo| is_heic(&photo.path)));
        let views: Vec<PhotoView> = photos.iter().map(|photo| photo_view(photo, SharedString::new())).collect();
        state.set_photos(ModelRc::new(VecModel::from(views)));
        state.set_loaded(true);
        let jobs = photos.into_iter().enumerate().map(|(index, photo)| (index, source(photo, download))).collect();
        images::load(app, jobs, PHOTO_SIZE, &GENERATION, set_photo);
    });
    // La lista para buscar se arma aparte: es solo leer la carpeta.
    bridge::spawn(app, || icloud::photos(SEARCHABLE), |_, photos| PHOTOS.set(photos));
}

/// Busca entre las fotos. El nombre y la fecha salen al instante; si se pide
/// algo visual ("azules", "oscuras"), las miniaturas se miran en segundo plano
/// y los resultados van apareciendo.
fn search(app: &AppWindow, text: String) {
    let generation = SEARCH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let query = photo_search::parse(&text);
    let state = app.global::<ICloudState>();
    if text.trim().is_empty() || query.is_empty() {
        state.set_search_note(SharedString::new());
        state.set_searching(false);
        show_found(app, Vec::new());
        return;
    }
    // Primero, lo que se puede decidir sin mirar las fotos.
    let candidates: Vec<CloudPhoto> =
        PHOTOS.with_borrow(|photos| photos.iter().filter(|photo| by_name_and_date(&query, photo)).cloned().collect());
    if !query.needs_looks() {
        state.set_searching(false);
        state.set_search_note(note(candidates.len(), candidates.len(), false, 0).into());
        show_found(app, candidates);
        return;
    }
    // Las que ya se miraron antes se pueden filtrar de una.
    let (known, pending): (Vec<CloudPhoto>, Vec<CloudPhoto>) = LOOKS.with_borrow(|cache| {
        candidates.iter().cloned().partition(|photo| cache.contains_key(&photo_search::key(photo)))
    });
    let matching: Vec<CloudPhoto> = LOOKS.with_borrow(|cache| {
        known.into_iter().filter(|photo| query.matches(photo, cache.get(&photo_search::key(photo)))).collect()
    });
    // Las fotos que están solo en la nube solo se pueden mirar si el usuario
    // permitió que iCloud las descargue para hacer su miniatura.
    let download = prefs::get().icloud_thumbnails;
    let mut cloud_left = if download { CLOUD_LOOK_LIMIT } else { 0 };
    let mut skipped = 0;
    let pending: Vec<CloudPhoto> = pending
        .into_iter()
        .filter(|photo| {
            if !photo.cloud_only {
                return true;
            }
            if cloud_left == 0 {
                skipped += 1;
                return false;
            }
            cloud_left -= 1;
            true
        })
        .collect();
    state.set_searching(!pending.is_empty());
    state.set_search_note(note(matching.len(), candidates.len(), !pending.is_empty(), skipped).into());
    show_found(app, matching.clone());
    if !pending.is_empty() {
        analyze(app, generation, text, pending, matching, candidates.len(), download, skipped);
    }
}

fn by_name_and_date(query: &photo_search::Query, photo: &CloudPhoto) -> bool {
    // Sin mirar la foto: alcanza para descartar por nombre y por fecha.
    let only_dates =
        photo_search::Query { names: query.names.clone(), looks: Vec::new(), year: query.year, month: query.month };
    only_dates.matches(photo, None)
}

/// Mira las miniaturas que faltan, de a tandas, y va sumando lo que coincide.
#[expect(clippy::too_many_arguments, reason = "el análisis necesita todo el contexto de la búsqueda")]
fn analyze(
    app: &AppWindow,
    generation: u32,
    text: String,
    pending: Vec<CloudPhoto>,
    found: Vec<CloudPhoto>,
    total: usize,
    download: bool,
    skipped: usize,
) {
    let weak = app.as_weak();
    let spawned = std::thread::Builder::new().name("liquidcontrol-fotos".into()).spawn(move || {
        let query = photo_search::parse(&text);
        let mut found = found;
        let mut looked = 0usize;
        for chunk in pending.chunks(BATCH) {
            if SEARCH_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let looks: Vec<(String, Option<Traits>)> =
                chunk.iter().map(|photo| (photo_search::key(photo), photo_search::analyze(photo, download))).collect();
            looked += chunk.len();
            let batch: Vec<CloudPhoto> = chunk
                .iter()
                .zip(&looks)
                .filter(|(photo, (_, traits))| query.matches(photo, traits.as_ref()))
                .map(|(photo, _)| photo.clone())
                .collect();
            found.extend(batch);
            let done = looked == pending.len();
            let snapshot = found.clone();
            let _ = weak.upgrade_in_event_loop(move |app| {
                if SEARCH_GENERATION.load(Ordering::SeqCst) != generation {
                    return;
                }
                LOOKS.with_borrow_mut(|cache| {
                    for (key, traits) in looks {
                        if let Some(traits) = traits {
                            cache.insert(key, traits);
                        }
                    }
                });
                let state = app.global::<ICloudState>();
                state.set_searching(!done);
                state.set_search_note(note(snapshot.len(), total, !done, skipped).into());
                show_found(&app, snapshot);
            });
        }
    });
    if let Err(err) = spawned {
        log::error!("no se pudo mirar las fotos: {err}");
    }
}

fn note(found: usize, total: usize, searching: bool, skipped: usize) -> String {
    let aside = if skipped > 0 {
        format!(
            " {skipped} están solo en iCloud y quedaron afuera: mirarlas obliga a iCloud a descargarlas (se permite con «Mostrar las fotos que están solo en iCloud», y como mucho {CLOUD_LOOK_LIMIT} por búsqueda)."
        )
    } else {
        String::new()
    };
    if searching {
        return format!("Mirando tus fotos… {found} de {total} hasta ahora.{aside}");
    }
    match found {
        0 => format!(
            "Ninguna foto coincide. Probá con parte del nombre, un color («azules»), «oscuras», «verticales» o un mes y año.{aside}"
        ),
        1 => format!("1 foto encontrada.{aside}"),
        n if n > MAX_FOUND => format!("{n} fotos encontradas (se muestran las {MAX_FOUND} más nuevas).{aside}"),
        n => format!("{n} fotos encontradas.{aside}"),
    }
}

/// Muestra los resultados con su miniatura.
fn show_found(app: &AppWindow, photos: Vec<CloudPhoto>) {
    let download = prefs::get().icloud_thumbnails;
    let photos: Vec<CloudPhoto> = photos.into_iter().take(MAX_FOUND).collect();
    let views: Vec<PhotoView> = photos.iter().map(|photo| photo_view(photo, photo.name.as_str().into())).collect();
    app.global::<ICloudState>().set_found(ModelRc::new(VecModel::from(views)));
    let jobs = photos.iter().cloned().enumerate().map(|(index, photo)| (index, source(photo, download))).collect();
    FOUND.set(photos);
    images::load(app, jobs, PHOTO_SIZE, &FOUND_GENERATION, set_found_photo);
}

fn set_found_photo(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let photos = app.global::<ICloudState>().get_found();
    if let Some(mut photo) = photos.row_data(index) {
        photo.image = images::image(pixels);
        photo.loaded = true;
        photos.set_row_data(index, photo);
    }
}

/// Al salir de la página se sueltan las miniaturas y se corta lo que se esté
/// analizando (las fotos ya miradas quedan en memoria para la próxima).
pub fn leave(app: &AppWindow) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    FOUND_GENERATION.fetch_add(1, Ordering::SeqCst);
    SEARCH_GENERATION.fetch_add(1, Ordering::SeqCst);
    let state = app.global::<ICloudState>();
    state.set_photos(ModelRc::default());
    state.set_found(ModelRc::default());
    state.set_search_note(SharedString::new());
    state.set_searching(false);
    PHOTOS.with_borrow_mut(Vec::clear);
    FOUND.with_borrow_mut(Vec::clear);
}

/// Sin permiso, una foto que está solo en la nube usa la miniatura guardada (nunca descarga).
fn source(photo: CloudPhoto, download: bool) -> Source {
    if photo.cloud_only && !download { Source::CachedThumbnail(photo.path) } else { Source::Thumbnail(photo.path) }
}

/// Las fotos del iPhone (HEIC) necesitan el códec gratuito de Microsoft para verse.
fn photo_view(photo: &CloudPhoto, name: SharedString) -> PhotoView {
    PhotoView {
        image: Image::default(),
        loaded: false,
        cloud_only: photo.cloud_only,
        name,
        path: photo.path.to_string_lossy().as_ref().into(),
    }
}

/// Vuelve a cargar lo que esté mostrando la subpágina actual (después de
/// borrar o descargar una foto desde el menú contextual).
fn refresh_current(app: &AppWindow) {
    match app.global::<Nav>().get_sub().as_str() {
        "icloud-photos" => enter_all(app),
        _ => enter(app),
    }
}

/// Al entrar a "Ver todas": las más nuevas, hasta el tope de seguridad.
pub fn enter_all(app: &AppWindow) {
    let generation = ALL_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let download = prefs::get().icloud_thumbnails;
    let state = app.global::<ICloudState>();
    state.set_loading_all_photos(true);
    state.set_all_photos(ModelRc::default());
    bridge::spawn(
        app,
        || {
            let photos = icloud::photos(usize::MAX);
            let total = photos.len();
            (photos, total)
        },
        move |app, (photos, total)| {
            if ALL_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let state = app.global::<ICloudState>();
            state.set_loading_all_photos(false);
            state.set_all_photos_total(i32::try_from(total).unwrap_or(i32::MAX));
            let photos: Vec<CloudPhoto> = photos.into_iter().take(ALL_PHOTOS_LIMIT).collect();
            let views: Vec<PhotoView> =
                photos.iter().map(|photo| photo_view(photo, photo.name.as_str().into())).collect();
            state.set_all_photos(ModelRc::new(VecModel::from(views)));
            let jobs =
                photos.iter().cloned().enumerate().map(|(index, photo)| (index, source(photo, download))).collect();
            images::load(app, jobs, ALL_PHOTOS_THUMB_SIZE, &ALL_GENERATION, set_all_photo);
        },
    );
}

pub fn leave_all(app: &AppWindow) {
    ALL_GENERATION.fetch_add(1, Ordering::SeqCst);
    let state = app.global::<ICloudState>();
    state.set_all_photos(ModelRc::default());
    state.set_search_note(SharedString::new());
    state.set_found(ModelRc::default());
}

fn set_all_photo(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let photos = app.global::<ICloudState>().get_all_photos();
    if let Some(mut photo) = photos.row_data(index) {
        photo.image = images::image(pixels);
        photo.loaded = true;
        photos.set_row_data(index, photo);
    }
}

fn is_heic(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif"))
}

fn set_photo(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let photos = app.global::<ICloudState>().get_photos();
    if let Some(mut photo) = photos.row_data(index) {
        photo.image = images::image(pixels);
        photo.loaded = true;
        photos.set_row_data(index, photo);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_heic;

    #[test]
    fn detects_iphone_photos() {
        assert!(is_heic(Path::new(r"C:\Fotos\IMG_0001.HEIC")));
        assert!(!is_heic(Path::new(r"C:\Fotos\IMG_0001.jpg")));
    }
}

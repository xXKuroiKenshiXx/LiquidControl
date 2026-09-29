//! Imágenes: conversión de lo que entrega el sistema a imágenes de Slint y carga
//! en segundo plano (miniaturas de fotos, íconos de archivos, de apps y del catálogo).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use liquidcontrol_core::media::{self, Thumbnail};
use liquidcontrol_core::wallpaper_search::{self, OnlineWallpaper};
use liquidcontrol_core::{AppResult, icons};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::AppWindow;

/// Pocos hilos: alcanzan para que las listas se llenen rápido sin saturar el disco.
const WORKERS: usize = 3;

/// Píxeles que pueden cruzar de hilo (`slint::Image` no es `Send`; el búfer sí).
pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// De dónde sale una imagen.
pub enum Source {
    /// Miniatura de una foto (si está solo en la nube, generarla la descarga).
    Thumbnail(PathBuf),
    /// Miniatura de una foto local; si no se puede leer no hay imagen (en vez
    /// de un ícono genérico estirado).
    Photo(PathBuf),
    /// Miniatura solo si ya está guardada: nunca descarga nada.
    CachedThumbnail(PathBuf),
    /// Ícono de un ejecutable, acceso directo o archivo `.ico`.
    FileIcon(PathBuf),
    /// Ícono de una app por su AppUserModelID (apps de la Store y de escritorio).
    AppIcon(String),
    /// Ícono público del sitio oficial de una app del catálogo (queda en caché).
    CatalogIcon { id: String, site: String },
    /// Miniatura de un resultado de búsqueda de fondos en línea (se descarga
    /// y queda en caché la primera vez).
    OnlineWallpaper(OnlineWallpaper),
}

impl Source {
    fn render(&self, size: u32) -> AppResult<Arc<Thumbnail>> {
        match self {
            Self::Thumbnail(path) => media::thumbnail(path, size),
            Self::Photo(path) => media::photo_thumbnail(path, size),
            Self::CachedThumbnail(path) => media::thumbnail_cached(path, size),
            Self::FileIcon(path) => media::file_icon(path, size),
            Self::AppIcon(aumid) => media::app_icon(aumid, size),
            Self::CatalogIcon { id, site } => media::picture(&icons::fetch(id, site)?, size),
            Self::OnlineWallpaper(item) => media::thumbnail(&wallpaper_search::thumb_path(item)?, size),
        }
    }
}

pub fn pixels(thumb: &Thumbnail) -> Pixels {
    SharedPixelBuffer::clone_from_slice(&thumb.rgba, thumb.width, thumb.height)
}

/// Se llama en el hilo de la interfaz.
pub fn image(pixels: Pixels) -> Image {
    Image::from_rgba8(pixels)
}

/// Carga imágenes de `size` px en segundo plano y entrega cada una a `apply` en
/// el hilo de la interfaz. Cada llamada incrementa `generation`: los resultados de
/// una carga anterior (o de un panel que ya se cerró) se descartan.
pub fn load<K: Send + 'static>(
    app: &AppWindow,
    jobs: Vec<(K, Source)>,
    size: u32,
    generation: &'static AtomicU32,
    apply: fn(&AppWindow, K, Pixels),
) {
    let current = generation.fetch_add(1, Ordering::SeqCst) + 1;
    if jobs.is_empty() {
        return;
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(jobs)));
    for _ in 0..WORKERS {
        let queue = Arc::clone(&queue);
        let weak = app.as_weak();
        let spawned = std::thread::Builder::new().name("liquidcontrol-imagenes".into()).spawn(move || {
            while generation.load(Ordering::SeqCst) == current {
                let next = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).pop_front();
                let Some((key, source)) = next else { break };
                // Sin imagen (ilegible, sin códec o sin conexión): queda el ícono genérico.
                let Ok(thumb) = source.render(size) else { continue };
                let pixels = pixels(&thumb);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if generation.load(Ordering::SeqCst) == current {
                        apply(&app, key, pixels);
                    }
                });
            }
        });
        if let Err(err) = spawned {
            log::error!("no se pudo crear un hilo de imágenes: {err}");
        }
    }
}

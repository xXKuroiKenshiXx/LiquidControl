//! Miniaturas de imágenes. El shell de Windows (con su caché de miniaturas)
//! genera el bitmap y WIC lo convierte a píxeles RGBA listos para la interfaz:
//! funciona con todo lo que Windows sabe abrir (JPEG, PNG, WebP, HEIC con su
//! extensión, RAW…) y sin dependencias extra.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, HPALETTE};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapIgnoreAlpha, WICBitmapPaletteTypeCustom,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_ICONONLY, SIIGBF_INCACHEONLY,
    SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY,
};
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::com;

const IMAGE_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "jfif", "png", "bmp", "gif", "webp", "heic", "heif", "avif", "jxl", "tif", "tiff"];
/// Tope de memoria de la caché de miniaturas (la app apunta a < 100 MB en total).
const CACHE_BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// Imagen decodificada en RGBA de 8 bits por canal.
#[derive(Debug)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Arc<Thumbnail>>,
    order: VecDeque<String>,
    bytes: usize,
}

impl Cache {
    fn insert(&mut self, key: String, thumb: Arc<Thumbnail>) {
        self.bytes += thumb.rgba.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, thumb);
        while self.bytes > CACHE_BUDGET_BYTES {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.rgba.len();
            }
        }
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

unsafe fn to_rgba(bitmap: HBITMAP) -> AppResult<Thumbnail> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let source = wic.CreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), WICBitmapIgnoreAlpha)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &source,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut width, mut height) = (0u32, 0u32);
        converter.GetSize(&mut width, &mut height)?;
        let stride = width * 4;
        let mut rgba = vec![0u8; (stride * height) as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut rgba)?;
        Ok(Thumbnail { width, height, rgba })
    }
}

/// Genera la imagen de un elemento del shell (archivo o app) en un hilo STA.
fn render(parsing_name: HSTRING, size: u32, flags: SIIGBF) -> AppResult<Thumbnail> {
    com::sta(move || unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&parsing_name, None)?;
        let side = size as i32;
        let bitmap = item.GetImage(SIZE { cx: side, cy: side }, flags)?;
        let result = to_rgba(bitmap);
        let _ = DeleteObject(bitmap.into());
        result
    })
}

fn modified_secs(path: &Path) -> AppResult<u64> {
    Ok(fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs()))
}

/// Busca en la caché o genera y guarda.
fn cached(key: String, make: impl FnOnce() -> AppResult<Thumbnail>) -> AppResult<Arc<Thumbnail>> {
    if let Some(hit) = cache().lock().unwrap_or_else(|p| p.into_inner()).entries.get(&key) {
        return Ok(Arc::clone(hit));
    }
    let thumb = Arc::new(make()?);
    cache().lock().unwrap_or_else(|p| p.into_inner()).insert(key, Arc::clone(&thumb));
    Ok(thumb)
}

fn file_render(path: &Path, size: u32, flags: SIIGBF, tag: &str) -> AppResult<Arc<Thumbnail>> {
    let size = size.clamp(16, 1024);
    let key = format!("{tag}|{}|{size}|{}", path.display(), modified_secs(path)?);
    let name = HSTRING::from(path.as_os_str());
    cached(key, move || render(name, size, flags))
}

/// Miniatura de una imagen del disco que entra en un cuadro de `size`×`size`.
/// Si el archivo está solo en la nube, generarla lo descarga.
pub fn thumbnail(path: &Path, size: u32) -> AppResult<Arc<Thumbnail>> {
    if !path.is_absolute() || !is_image(path) || !path.is_file() {
        return Err(AppError::invalid("Solo se pueden previsualizar imágenes."));
    }
    file_render(path, size.max(32), SIIGBF_RESIZETOFIT, "img")
}

/// Miniatura real de una foto local. A diferencia de [`thumbnail`], si Windows
/// no sabe leerla (por ejemplo un HEIC sin el códec) falla en vez de devolver
/// un ícono genérico, así la interfaz puede mostrar su propio ícono.
pub fn photo_thumbnail(path: &Path, size: u32) -> AppResult<Arc<Thumbnail>> {
    if !path.is_absolute() || !is_image(path) || !path.is_file() {
        return Err(AppError::invalid("Solo se pueden previsualizar imágenes."));
    }
    file_render(path, size.max(32), SIIGBF_THUMBNAILONLY, "photo")
}

/// Miniatura solo si Windows (o el proveedor de la nube) ya la tiene guardada:
/// nunca descarga el archivo.
pub fn thumbnail_cached(path: &Path, size: u32) -> AppResult<Arc<Thumbnail>> {
    if !path.is_absolute() || !is_image(path) {
        return Err(AppError::invalid("Solo se pueden previsualizar imágenes."));
    }
    file_render(path, size.max(32), SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY, "cache")
}

/// Imagen de un archivo `.png`/`.ico` descargado (íconos del catálogo).
pub fn picture(path: &Path, size: u32) -> AppResult<Arc<Thumbnail>> {
    let extension = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase);
    if !path.is_absolute() || !matches!(extension.as_deref(), Some("png" | "ico")) || !path.is_file() {
        return Err(AppError::invalid("Formato de ícono no admitido."));
    }
    file_render(path, size, SIIGBF_RESIZETOFIT, "pic")
}

/// Ícono de un archivo (el de un ejecutable, un acceso directo o un `.ico`).
pub fn file_icon(path: &Path, size: u32) -> AppResult<Arc<Thumbnail>> {
    if !path.is_absolute() || !path.exists() {
        return Err(AppError::not_found("El archivo no existe."));
    }
    file_render(path, size, SIIGBF_ICONONLY, "icon")
}

/// Ícono de una app instalada por su AppUserModelID (apps de la Store y de escritorio).
pub fn app_icon(aumid: &str, size: u32) -> AppResult<Arc<Thumbnail>> {
    if aumid.is_empty() || aumid.len() > 512 || aumid.chars().any(|c| c.is_control() || c == '/' || c == '?') {
        return Err(AppError::invalid("Identificador de app inválido."));
    }
    let size = size.clamp(16, 256);
    let name = HSTRING::from(format!(r"shell:AppsFolder\{aumid}"));
    cached(format!("aumid|{aumid}|{size}"), move || render(name, size, SIIGBF_ICONONLY))
}

//! Búsqueda de fondos en [Openverse](https://openverse.org) (openverse.org),
//! el buscador de contenido con licencia Creative Commons de la Fundación
//! Openverse (antes parte de WordPress.org): reúne fotos de Flickr, Wikimedia
//! Commons, museos y más, siempre con licencia abierta. Sin clave para el uso
//! anónimo (200 pedidos por día alcanzan de sobra para esto). Es una de las
//! dos fuentes de [`crate::wallpaper_search`], que es quien arma la consulta
//! (traducida) y junta los resultados.

use serde::Deserialize;

use crate::error::AppResult;
use crate::net;
use crate::wallpaper_search::OnlineWallpaper;

const MAX_SEARCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESULTS: usize = 18;

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    id: String,
    url: String,
    thumbnail: String,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// Codifica los caracteres que rompen una URL; igual que en `wallhaven.rs`.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Busca fotos por texto. `mature=false` (por defecto) excluye contenido para
/// adultos; `license_type=all-cc` acepta cualquier licencia Creative Commons
/// (todas permiten usar la imagen como fondo de escritorio personal).
pub fn search(query: &str) -> AppResult<Vec<OnlineWallpaper>> {
    let url = format!(
        "https://api.openverse.org/v1/images/?q={}&category=photograph&license_type=all-cc&mature=false&page_size={MAX_RESULTS}",
        encode(query)
    );
    let bytes = net::get(&url, MAX_SEARCH_BYTES)?;
    let response: SearchResponse = serde_json::from_slice(&bytes)?;
    Ok(response
        .results
        .into_iter()
        .filter(|item| !item.url.is_empty() && !item.thumbnail.is_empty())
        .take(MAX_RESULTS)
        .map(|item| OnlineWallpaper {
            id: format!("ov-{}", item.id),
            resolution: match (item.width, item.height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => format!("{w}x{h}"),
                _ => String::new(),
            },
            thumb_url: item.thumbnail,
            full_url: item.url,
            source: "Openverse",
        })
        .collect())
}

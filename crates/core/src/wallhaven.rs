//! Búsqueda de fondos en Wallhaven (wallhaven.cc), un sitio de fondos hechos
//! y subidos por su comunidad, sin necesidad de una clave para búsquedas
//! básicas. Se filtra siempre a contenido "sfw" (apto para todo público).
//! Es una de las dos fuentes de [`crate::wallpaper_search`], que es quien
//! arma la consulta (traducida) y junta los resultados.

use serde::Deserialize;

use crate::error::AppResult;
use crate::net;
use crate::wallpaper_search::OnlineWallpaper;

const MAX_SEARCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESULTS: usize = 18;

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    id: String,
    resolution: String,
    path: String,
    thumbs: RawThumbs,
}

#[derive(Deserialize)]
struct RawThumbs {
    small: String,
}

/// Codifica los caracteres que rompen una URL (espacios, tildes, etc.); deja
/// pasar letras, números y `-_.~` tal cual, que es lo único que hace falta
/// para una búsqueda de texto simple.
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

/// Busca fondos por texto. Como máximo 18 resultados, todos aptos para todo público.
pub fn search(query: &str) -> AppResult<Vec<OnlineWallpaper>> {
    let url =
        format!("https://wallhaven.cc/api/v1/search?q={}&categories=111&purity=100&sorting=relevance", encode(query));
    let bytes = net::get(&url, MAX_SEARCH_BYTES)?;
    let response: SearchResponse = serde_json::from_slice(&bytes)?;
    Ok(response
        .data
        .into_iter()
        .take(MAX_RESULTS)
        .map(|item| OnlineWallpaper {
            id: format!("wh-{}", item.id),
            resolution: item.resolution,
            thumb_url: item.thumbs.small,
            full_url: item.path,
            source: "Wallhaven",
        })
        .collect())
}

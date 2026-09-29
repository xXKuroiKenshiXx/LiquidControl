//! Buscador de las fotos de iCloud: por nombre, por fecha y por **cómo se ve**
//! la foto (color que domina, si es clara u oscura, si está en blanco y negro y
//! si es vertical u horizontal).
//!
//! No hay modelo de IA de por medio: eso pediría cientos de MB de RAM y la app
//! apunta a menos de 100. En cambio se mira la miniatura que Windows ya tiene
//! guardada: de ahí salen el tono dominante, la saturación, el brillo y la
//! forma. Alcanza para "fotos azules", "fotos oscuras" o "verticales", y todo
//! se calcula en esta PC, sin subir nada a ningún lado. Las fotos que están
//! solo en la nube usan la miniatura ya guardada: nunca se fuerza su descarga.

use std::collections::HashMap;

use liquidcontrol_core::icloud::CloudPhoto;
use liquidcontrol_core::media;

/// Cómo se ve una foto, resumido en cuatro números.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Traits {
    /// Tono dominante en grados (0 = rojo, 120 = verde, 240 = azul).
    pub hue: f32,
    /// Qué tan vivo es el color (0 = gris, 1 = saturado).
    pub saturation: f32,
    /// Qué tan clara es (0 = negra, 1 = blanca).
    pub light: f32,
    /// Más alta que ancha.
    pub portrait: bool,
    /// Casi cuadrada.
    pub square: bool,
}

/// Lo que se pide en la búsqueda, ya interpretado.
#[derive(Debug, Default, PartialEq)]
pub struct Query {
    /// Palabras que se buscan en el nombre del archivo.
    pub names: Vec<String>,
    /// Condiciones sobre cómo se ve la foto.
    pub looks: Vec<Look>,
    /// Año (2024) y mes (1–12) pedidos.
    pub year: Option<i32>,
    pub month: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Tono dominante entre estos grados (puede cruzar el 0).
    Hue(u16, u16),
    Dark,
    Bright,
    Gray,
    Colorful,
    White,
    Black,
    Portrait,
    Landscape,
    Square,
    Screenshot,
}

const MONTHS: [(&str, u32); 12] = [
    ("enero", 1),
    ("febrero", 2),
    ("marzo", 3),
    ("abril", 4),
    ("mayo", 5),
    ("junio", 6),
    ("julio", 7),
    ("agosto", 8),
    ("septiembre", 9),
    ("octubre", 10),
    ("noviembre", 11),
    ("diciembre", 12),
];

/// Palabras que describen cómo se ve una foto.
fn look_for(word: &str) -> Option<Look> {
    Some(match word {
        "roja" | "rojas" | "rojo" | "rojos" => Look::Hue(345, 15),
        "naranja" | "naranjas" => Look::Hue(15, 45),
        "amarilla" | "amarillas" | "amarillo" | "amarillos" => Look::Hue(45, 70),
        "verde" | "verdes" => Look::Hue(70, 165),
        "celeste" | "celestes" | "turquesa" => Look::Hue(165, 200),
        "azul" | "azules" => Look::Hue(200, 255),
        "violeta" | "violetas" | "morada" | "moradas" | "lila" => Look::Hue(255, 290),
        "rosa" | "rosas" | "rosada" | "rosadas" => Look::Hue(290, 345),
        "oscura" | "oscuras" | "oscuro" | "oscuros" | "noche" => Look::Dark,
        "clara" | "claras" | "claro" | "claros" | "luminosa" | "luminosas" => Look::Bright,
        "gris" | "grises" | "byn" | "monocromo" => Look::Gray,
        "colorida" | "coloridas" | "colorido" | "coloridos" => Look::Colorful,
        "blanca" | "blancas" | "blanco" => Look::White,
        "negra" | "negras" | "negro" => Look::Black,
        "vertical" | "verticales" | "retrato" => Look::Portrait,
        "horizontal" | "horizontales" | "apaisada" | "panoramica" => Look::Landscape,
        "cuadrada" | "cuadradas" | "cuadrado" => Look::Square,
        "captura" | "capturas" | "screenshot" | "screenshots" | "pantallazo" => Look::Screenshot,
        _ => return None,
    })
}

/// "fotos azules de enero 2024" → colores, mes y año.
pub fn parse(query: &str) -> Query {
    let mut parsed = Query::default();
    for raw in query.split(|c: char| !c.is_alphanumeric()) {
        let word = crate::search::normalize(raw);
        if word.is_empty() || matches!(word.as_str(), "foto" | "fotos" | "imagen" | "imagenes" | "de" | "del" | "y") {
            continue;
        }
        if let Ok(year) = word.parse::<i32>() {
            if (1990..=2100).contains(&year) {
                parsed.year = Some(year);
                continue;
            }
        }
        if let Some((_, month)) = MONTHS.iter().find(|(name, _)| *name == word) {
            parsed.month = Some(*month);
            continue;
        }
        if let Some(look) = look_for(&word) {
            parsed.looks.push(look);
            continue;
        }
        parsed.names.push(word);
    }
    // "blanco y negro" no son dos colores: es una foto sin color.
    if parsed.looks.contains(&Look::White) && parsed.looks.contains(&Look::Black) {
        parsed.looks.retain(|look| !matches!(look, Look::White | Look::Black));
        parsed.looks.push(Look::Gray);
    }
    parsed
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.looks.is_empty() && self.year.is_none() && self.month.is_none()
    }

    /// ¿Hace falta mirar las fotos (y no solo su nombre y su fecha)?
    pub fn needs_looks(&self) -> bool {
        self.looks.iter().any(|look| !matches!(look, Look::Screenshot))
    }

    /// ¿Esta foto cumple con todo lo que se pidió?
    pub fn matches(&self, photo: &CloudPhoto, traits: Option<&Traits>) -> bool {
        let name = crate::search::normalize(&photo.name);
        if !self.names.iter().all(|word| name.contains(word.as_str())) {
            return false;
        }
        if let Some(year) = self.year {
            if year_of(photo.modified) != year {
                return false;
            }
        }
        if let Some(month) = self.month {
            if month_of(photo.modified) != month {
                return false;
            }
        }
        self.looks.iter().all(|look| match look {
            Look::Screenshot => name.contains("captura") || name.contains("screenshot"),
            look => traits.is_some_and(|traits| traits.is(*look)),
        })
    }
}

impl Traits {
    fn is(&self, look: Look) -> bool {
        match look {
            Look::Hue(from, to) => {
                let hue = self.hue;
                let (from, to) = (f32::from(from), f32::from(to));
                let inside = if from <= to { hue >= from && hue < to } else { hue >= from || hue < to };
                inside && self.saturation >= 0.18 && self.light > 0.08 && self.light < 0.92
            }
            Look::Dark => self.light < 0.35,
            Look::Bright => self.light > 0.62,
            Look::Gray => self.saturation < 0.12,
            Look::Colorful => self.saturation > 0.45,
            Look::White => self.light > 0.82 && self.saturation < 0.15,
            Look::Black => self.light < 0.16,
            Look::Portrait => self.portrait && !self.square,
            Look::Landscape => !self.portrait && !self.square,
            Look::Square => self.square,
            Look::Screenshot => false,
        }
    }
}

/// Mira la miniatura y saca el tono, la saturación, el brillo y la forma.
///
/// Una foto que está solo en la nube no tiene miniatura para mirar: generarla
/// obliga a iCloud a descargarla. Por eso solo se hace si el usuario lo
/// permitió (`download`, el mismo permiso que ya existe para ver las
/// miniaturas); si no, esa foto queda sin analizar.
pub fn analyze(photo: &CloudPhoto, download: bool) -> Option<Traits> {
    const SIZE: u32 = 64;
    let thumb = if photo.cloud_only && !download {
        media::thumbnail_cached(&photo.path, SIZE).ok()?
    } else {
        media::thumbnail(&photo.path, SIZE).ok()?
    };
    let pixels: Vec<[f32; 3]> = thumb
        .rgba
        .chunks_exact(4)
        .filter(|px| px[3] > 32)
        .map(|px| [f32::from(px[0]) / 255.0, f32::from(px[1]) / 255.0, f32::from(px[2]) / 255.0])
        .collect();
    if pixels.is_empty() {
        return None;
    }
    // El tono se vota en 36 casilleros, pesando cada píxel por lo vivo que es:
    // un cielo azul gana aunque haya muchos grises alrededor.
    let mut bins = [0f32; 36];
    let (mut saturation, mut light) = (0f32, 0f32);
    for [r, g, b] in &pixels {
        let (hue, sat, lum) = hsl(*r, *g, *b);
        saturation += sat;
        light += lum;
        let bin = ((hue / 10.0) as usize).min(35);
        bins[bin] += sat * (1.0 - (lum - 0.5).abs());
    }
    let count = pixels.len() as f32;
    let best = bins.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
    let aspect = thumb.width as f32 / thumb.height.max(1) as f32;
    Some(Traits {
        hue: best as f32 * 10.0 + 5.0,
        saturation: saturation / count,
        light: light / count,
        portrait: aspect < 1.0,
        square: (aspect - 1.0).abs() < 0.08,
    })
}

/// RGB → tono (0–360), saturación y luminosidad (0–1).
fn hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) / 2.0;
    let delta = max - min;
    if delta < f32::EPSILON {
        return (0.0, 0.0, light);
    }
    let saturation = delta / (1.0 - (2.0 * light - 1.0).abs()).max(f32::EPSILON);
    let hue = if (max - r).abs() < f32::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    ((hue + 360.0) % 360.0, saturation.clamp(0.0, 1.0), light)
}

/// Año de una fecha en segundos desde 1970 (cuentas simples, sin dependencias).
fn year_of(seconds: u64) -> i32 {
    civil(seconds).0
}

fn month_of(seconds: u64) -> u32 {
    civil(seconds).1
}

/// Segundos desde 1970 → (año, mes). Algoritmo de Howard Hinnant (`civil_from_days`).
fn civil(seconds: u64) -> (i32, u32) {
    let days = (seconds / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    ((year + i64::from(month <= 2)) as i32, month as u32)
}

/// Guarda lo analizado para no volver a mirar las mismas fotos.
pub type Cache = HashMap<String, Traits>;

/// Clave de caché: la ruta y su fecha, así una foto editada se vuelve a mirar.
pub fn key(photo: &CloudPhoto) -> String {
    format!("{}|{}", photo.path.display(), photo.modified)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Look, Traits, civil, parse};
    use liquidcontrol_core::icloud::CloudPhoto;

    fn photo(name: &str, modified: u64) -> CloudPhoto {
        CloudPhoto { path: PathBuf::from(name), name: name.to_owned(), cloud_only: false, modified }
    }

    #[test]
    fn reads_colors_dates_and_names() {
        let query = parse("fotos azules de enero 2024");
        assert_eq!(query.looks, vec![Look::Hue(200, 255)]);
        assert_eq!(query.year, Some(2024));
        assert_eq!(query.month, Some(1));
        assert!(query.names.is_empty());

        let query = parse("IMG_0042");
        assert_eq!(query.names, vec!["img", "0042"]);
        assert!(!query.needs_looks());
    }

    #[test]
    fn matches_by_name_and_date() {
        // 2024-01-15 12:00 UTC
        let photo = photo("IMG_0042.HEIC", 1_705_320_000);
        assert!(parse("img_0042").matches(&photo, None));
        assert!(parse("0042 enero 2024").matches(&photo, None));
        assert!(!parse("0042 marzo").matches(&photo, None));
        assert!(!parse("otra").matches(&photo, None));
    }

    #[test]
    fn black_and_white_is_not_two_colors() {
        assert_eq!(parse("blanco y negro").looks, vec![Look::Gray]);
    }

    #[test]
    fn matches_by_how_it_looks() {
        let blue = Traits { hue: 215.0, saturation: 0.6, light: 0.45, portrait: false, square: false };
        let dark_gray = Traits { hue: 0.0, saturation: 0.03, light: 0.2, portrait: true, square: false };
        let photo = photo("IMG_1.HEIC", 1_705_320_000);
        assert!(parse("azules").matches(&photo, Some(&blue)));
        assert!(!parse("azules").matches(&photo, Some(&dark_gray)));
        assert!(parse("oscuras verticales").matches(&photo, Some(&dark_gray)));
        assert!(parse("blanco y negro").matches(&photo, Some(&dark_gray)));
        assert!(!parse("coloridas").matches(&photo, Some(&dark_gray)));
        // Sin haber mirado la foto, una búsqueda visual no puede coincidir.
        assert!(!parse("azules").matches(&photo, None));
    }

    #[test]
    fn screenshots_are_found_by_name() {
        let shot = photo("Captura de pantalla 2024.png", 1_705_320_000);
        assert!(parse("capturas").matches(&shot, None));
        assert!(!parse("capturas").matches(&photo("IMG_1.HEIC", 1_705_320_000), None));
    }

    #[test]
    fn converts_dates() {
        assert_eq!(civil(1_705_320_000), (2024, 1));
        assert_eq!(civil(0), (1970, 1));
        assert_eq!(civil(1_767_225_600), (2026, 1));
    }
}

#[cfg(test)]
mod bench {
    use super::{analyze, parse};

    #[test]
    #[ignore = "usa las fotos reales de iCloud; correr con --ignored --nocapture"]
    fn looks_at_real_photos() {
        let photos = liquidcontrol_core::icloud::photos(120);
        println!("fotos listadas: {}", photos.len());
        let started = std::time::Instant::now();
        let cloud = photos.iter().filter(|p| p.cloud_only).count();
        println!("solo en la nube: {cloud}");
        let looks: Vec<_> = photos.iter().map(|photo| (photo, analyze(photo, false))).collect();
        let ok = looks.iter().filter(|(_, t)| t.is_some()).count();
        println!(
            "analizadas {} de {} en {:.2} s ({:.0} ms cada una)",
            ok,
            photos.len(),
            started.elapsed().as_secs_f64(),
            started.elapsed().as_secs_f64() * 1000.0 / photos.len().max(1) as f64
        );
        for query in ["azules", "oscuras", "claras", "verdes", "blanco y negro", "verticales", "coloridas"] {
            let parsed = parse(query);
            let hits = looks.iter().filter(|(p, t)| parsed.matches(p, t.as_ref())).count();
            println!("{query:>16} → {hits} de {}", photos.len());
        }
    }
}

//! Búsqueda de fondos de pantalla gratis: junta [`wallhaven`] (comunidad de
//! Wallhaven) y [`openverse`] (buscador de contenido con licencia Creative
//! Commons, que reúne Flickr, Wikimedia Commons y otros) en una sola lista.
//!
//! El motivo de por qué esto hace falta: los dos sitios son en inglés y
//! buscan por **coincidencia de palabra**, no por significado. "montaña" no
//! encuentra las fotos etiquetadas "mountain" — Wallhaven devuelve 2
//! resultados casi al azar en vez de las miles que tiene para "mountain". Así
//! que además de mandar la consulta tal cual la escribió el usuario, esta
//! traduce las palabras que reconoce (diccionario de [`SPANISH_TO_ENGLISH`],
//! los términos típicos de un buscador de fondos) y manda **las dos
//! consultas a los dos sitios, en paralelo**, y junta todo en una sola lista.

use std::thread;

use crate::error::AppResult;
use crate::{net, paths};

const MAX_THUMB_BYTES: usize = 2 * 1024 * 1024;
/// Total que se muestra, ya mezclado (cada sitio ya devuelve como mucho 18).
const MAX_RESULTS: usize = 32;

/// Palabras que no aportan nada a la búsqueda ("fondo de pantalla azul" → "azul").
const FILLER: [&str; 10] =
    ["fondo", "fondos", "pantalla", "pantallas", "imagen", "imagenes", "wallpaper", "wallpapers", "de", "para"];

/// Español → inglés para lo que se suele buscar en un fondo de pantalla:
/// naturaleza, animales, colores, clima, lugares, temas y estados de ánimo.
/// No es un traductor general — alcanza con que cubra estas búsquedas típicas.
const SPANISH_TO_ENGLISH: &[(&str, &str)] = &[
    // Naturaleza y paisajes
    ("montana", "mountain"),
    ("montanas", "mountains"),
    ("playa", "beach"),
    ("mar", "sea"),
    ("oceano", "ocean"),
    ("bosque", "forest"),
    ("selva", "jungle"),
    ("desierto", "desert"),
    ("nieve", "snow"),
    ("lluvia", "rain"),
    ("tormenta", "storm"),
    ("niebla", "fog"),
    ("cascada", "waterfall"),
    ("lago", "lake"),
    ("rio", "river"),
    ("isla", "island"),
    ("volcan", "volcano"),
    ("cueva", "cave"),
    ("campo", "field"),
    ("valle", "valley"),
    ("acantilado", "cliff"),
    ("glaciar", "glacier"),
    ("arcoiris", "rainbow"),
    // Cielo y espacio
    ("cielo", "sky"),
    ("nubes", "clouds"),
    ("nube", "cloud"),
    ("estrellas", "stars"),
    ("estrella", "star"),
    ("espacio", "space"),
    ("galaxia", "galaxy"),
    ("luna", "moon"),
    ("planeta", "planet"),
    ("sol", "sun"),
    ("amanecer", "sunrise"),
    ("atardecer", "sunset"),
    ("noche", "night"),
    ("aurora", "aurora"),
    ("universo", "universe"),
    ("nebulosa", "nebula"),
    // Animales
    ("gato", "cat"),
    ("gatos", "cats"),
    ("perro", "dog"),
    ("perros", "dogs"),
    ("lobo", "wolf"),
    ("zorro", "fox"),
    ("leon", "lion"),
    ("tigre", "tiger"),
    ("oso", "bear"),
    ("aguila", "eagle"),
    ("buho", "owl"),
    ("caballo", "horse"),
    ("pajaro", "bird"),
    ("pajaros", "birds"),
    ("pez", "fish"),
    ("peces", "fish"),
    ("tiburon", "shark"),
    ("ballena", "whale"),
    ("dragon", "dragon"),
    ("mariposa", "butterfly"),
    ("lobos", "wolves"),
    // Plantas
    ("flores", "flowers"),
    ("flor", "flower"),
    ("arbol", "tree"),
    ("arboles", "trees"),
    ("hojas", "leaves"),
    ("jardin", "garden"),
    ("cerezo", "cherry blossom"),
    ("rosas", "roses"),
    // Colores
    ("rojo", "red"),
    ("roja", "red"),
    ("azul", "blue"),
    ("verde", "green"),
    ("amarillo", "yellow"),
    ("naranja", "orange"),
    ("violeta", "purple"),
    ("morado", "purple"),
    ("rosa", "pink"),
    ("rosado", "pink"),
    ("negro", "black"),
    ("blanco", "white"),
    ("gris", "gray"),
    ("dorado", "gold"),
    ("plateado", "silver"),
    ("turquesa", "turquoise"),
    ("celeste", "cyan"),
    ("pastel", "pastel"),
    ("neon", "neon"),
    ("colorido", "colorful"),
    ("multicolor", "colorful"),
    ("monocromo", "monochrome"),
    ("blanconegro", "black and white"),
    // Luz y tono
    ("oscuro", "dark"),
    ("oscura", "dark"),
    ("claro", "light"),
    ("clara", "light"),
    ("brillante", "bright"),
    ("luminoso", "glowing"),
    // Estilo
    ("minimalista", "minimal"),
    ("minimalismo", "minimalist"),
    ("abstracto", "abstract"),
    ("geometrico", "geometric"),
    ("simple", "simple"),
    ("moderno", "modern"),
    ("vintage", "vintage"),
    ("retro", "retro"),
    ("elegante", "elegant"),
    ("degradado", "gradient"),
    ("textura", "texture"),
    ("patron", "pattern"),
    ("acuarela", "watercolor"),
    ("pixelado", "pixel art"),
    ("bokeh", "bokeh"),
    // Temas
    ("anime", "anime"),
    ("manga", "manga"),
    ("videojuego", "video game"),
    ("videojuegos", "gaming"),
    ("pelicula", "movie"),
    ("superheroe", "superhero"),
    ("futurista", "futuristic"),
    ("ciberpunk", "cyberpunk"),
    ("fantasia", "fantasy"),
    ("medieval", "medieval"),
    ("terror", "horror"),
    ("guerra", "war"),
    ("navidad", "christmas"),
    ("halloween", "halloween"),
    ("musica", "music"),
    ("deportes", "sports"),
    ("futbol", "soccer"),
    // Lugares y ciudad
    ("ciudad", "city"),
    ("ciudades", "cities"),
    ("edificio", "building"),
    ("edificios", "buildings"),
    ("calle", "street"),
    ("puente", "bridge"),
    ("castillo", "castle"),
    ("templo", "temple"),
    ("japon", "japan"),
    ("paris", "paris"),
    ("nevado", "snowy"),
    // Objetos y tecnología
    ("auto", "car"),
    ("autos", "cars"),
    ("carro", "car"),
    ("moto", "motorcycle"),
    ("avion", "airplane"),
    ("nave", "spaceship"),
    ("robot", "robot"),
    ("tecnologia", "technology"),
    ("circuito", "circuit board"),
    ("codigo", "code"),
    ("electronica", "electronic"),
    ("guitarra", "guitar"),
    // Ánimo / abstracto
    ("triste", "sad"),
    ("feliz", "happy"),
    ("calma", "calm"),
    ("tranquilo", "peaceful"),
    ("relajante", "relaxing"),
    ("misterioso", "mysterious"),
    ("elegancia", "elegance"),
    ("lujo", "luxury"),
    ("humo", "smoke"),
    ("fuego", "fire"),
    ("hielo", "ice"),
    ("agua", "water"),
    ("burbujas", "bubbles"),
    ("cristal", "crystal"),
    ("marmol", "marble"),
    ("madera", "wood"),
    ("piedra", "stone"),
    ("arena", "sand"),
    ("oro", "gold"),
];

#[derive(Debug, Clone)]
pub struct OnlineWallpaper {
    /// Único entre los dos sitios (prefijo `wh-`/`ov-`): sirve de clave de caché.
    pub id: String,
    pub resolution: String,
    pub thumb_url: String,
    pub full_url: String,
    /// De dónde salió, para el crédito ("Wallhaven" o "Openverse").
    pub source: &'static str,
}

/// Minúsculas y sin tildes (duplica `app::search::normalize`, pero este crate
/// no depende de la interfaz): lo único que hace falta para mirar el diccionario.
fn normalize_word(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => Some('a'),
            'é' | 'è' | 'ë' | 'ê' => Some('e'),
            'í' | 'ì' | 'ï' | 'î' => Some('i'),
            'ó' | 'ò' | 'ö' | 'ô' => Some('o'),
            'ú' | 'ù' | 'ü' | 'û' => Some('u'),
            'ñ' => Some('n'),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Busca la palabra tal cual, y si no está, probando sin una `s` o `es` del
/// final (para no tener que listar cada plural: "azules" → "azul" → "blue").
fn lookup(word: &str) -> Option<&'static str> {
    SPANISH_TO_ENGLISH
        .iter()
        .find(|(es, _)| *es == word)
        .or_else(|| {
            let stem = word.strip_suffix("es").or_else(|| word.strip_suffix('s'))?;
            SPANISH_TO_ENGLISH.iter().find(|(es, _)| *es == stem)
        })
        .map(|(_, en)| *en)
}

/// Traduce lo que reconoce del diccionario; `None` si no cambió nada (para no
/// repetir la misma búsqueda dos veces).
fn translate(query: &str) -> Option<String> {
    let words: Vec<String> = query.split_whitespace().map(normalize_word).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    let mut changed = false;
    let translated: Vec<&str> = words
        .iter()
        .filter(|word| !FILLER.contains(&word.as_str()))
        .map(|word| {
            lookup(word).map_or(word.as_str(), |en| {
                changed = true;
                en
            })
        })
        .collect();
    (changed && !translated.is_empty()).then(|| translated.join(" "))
}

/// Busca fondos por texto en Wallhaven y en Openverse. Si el texto tiene
/// palabras reconocidas en español, se manda también su traducción al inglés,
/// y las dos búsquedas (hasta 4 pedidos: 2 sitios × hasta 2 consultas) se
/// hacen en paralelo y se mezclan en una sola lista.
pub fn search(query: &str) -> AppResult<Vec<OnlineWallpaper>> {
    let query = query.trim();
    let translated = translate(query);
    let variants: Vec<&str> = std::iter::once(query).chain(translated.as_deref()).filter(|q| !q.is_empty()).collect();

    let mut sources: Vec<AppResult<Vec<OnlineWallpaper>>> = Vec::new();
    thread::scope(|scope| {
        let handles: Vec<_> = variants
            .iter()
            .flat_map(|&variant| {
                [
                    scope.spawn(move || crate::wallhaven::search(variant)),
                    scope.spawn(move || crate::openverse::search(variant)),
                ]
            })
            .collect();
        for handle in handles {
            if let Ok(result) = handle.join() {
                sources.push(result);
            }
        }
    });

    // Se junta todo lo que salió bien; si nada salió bien (por ejemplo, sin
    // conexión), se devuelve el primer error para que la interfaz lo muestre.
    let mut lists: Vec<Vec<OnlineWallpaper>> = Vec::new();
    let mut first_error = None;
    for result in sources {
        match result {
            Ok(list) => lists.push(list),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    if lists.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
        return Ok(Vec::new());
    }

    // Se intercalan (Wallhaven original, Openverse original, Wallhaven
    // traducido, Openverse traducido…) para que no queden todos los
    // resultados de un mismo sitio o una misma consulta juntos.
    let mut merged = Vec::with_capacity(MAX_RESULTS);
    let mut seen_ids = std::collections::HashSet::new();
    let mut cursors = vec![0usize; lists.len()];
    'fill: loop {
        let mut advanced = false;
        for (list, cursor) in lists.iter().zip(cursors.iter_mut()) {
            let Some(item) = list.get(*cursor) else { continue };
            *cursor += 1;
            advanced = true;
            if seen_ids.insert(item.id.clone()) {
                merged.push(item.clone());
                if merged.len() >= MAX_RESULTS {
                    break 'fill;
                }
            }
        }
        if !advanced {
            break;
        }
    }
    Ok(merged)
}

fn cache_dir() -> AppResult<std::path::PathBuf> {
    Ok(paths::data_dir()?.join("cache").join("wallpapers"))
}

fn extension(url: &str) -> &str {
    url.rsplit('.').next().filter(|ext| ext.len() <= 4 && ext.chars().all(char::is_alphanumeric)).unwrap_or("jpg")
}

/// Miniatura en caché (se descarga la primera vez que se pide).
pub fn thumb_path(item: &OnlineWallpaper) -> AppResult<std::path::PathBuf> {
    let path = cache_dir()?.join(format!("{}-thumb.{}", item.id, extension(&item.thumb_url)));
    if !path.exists() {
        let bytes = net::get(&item.thumb_url, MAX_THUMB_BYTES)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

/// Imagen completa en caché (se descarga al elegir aplicarla): esta ruta se
/// puede pasar directo a `wallpaper::set`, igual que un archivo elegido a mano.
pub fn download_full(item: &OnlineWallpaper) -> AppResult<std::path::PathBuf> {
    let path = cache_dir()?.join(format!("{}.{}", item.id, extension(&item.full_url)));
    if !path.exists() {
        net::download_to_file(&item.full_url, &path, |_, _| {})?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{normalize_word, translate};

    #[test]
    fn translates_known_spanish_words() {
        assert_eq!(translate("montaña"), Some("mountain".to_owned()));
        assert_eq!(translate("montañas azules"), Some("mountains blue".to_owned()));
        assert_eq!(translate("fondo de pantalla playa"), Some("beach".to_owned()));
    }

    #[test]
    fn leaves_english_or_unknown_words_alone() {
        assert_eq!(translate("mountain"), None);
        assert_eq!(translate("tokyo"), None);
        assert_eq!(translate(""), None);
    }

    #[test]
    fn normalizes_accents() {
        assert_eq!(normalize_word("MONTAÑA"), "montana");
        assert_eq!(normalize_word("océano"), "oceano");
    }
}

//! Búsqueda de la barra lateral. Encuentra paneles, opciones sueltas y cada
//! uno de los ajustes del catálogo (los de Privacidad, Escritorio, Juegos…),
//! escribiendo frases enteras ("como desactivo copilot"), en inglés
//! ("dark mode") y con errores de tipeo ("blutooth").
//!
//! Cómo funciona:
//! 1. La consulta se normaliza (sin tildes, guiones ni mayúsculas), se le sacan
//!    las palabras de relleno ("como", "el", "para"…) y cada palabra se expande
//!    con sus sinónimos ([`SYNONYMS`]).
//! 2. Cada entrada del índice se puntúa palabra por palabra contra su título,
//!    su sección, sus palabras clave y su descripción, con puntaje decreciente
//!    según dónde aparece y si fue coincidencia exacta, por prefijo o tolerando
//!    un error de tipeo ([`fuzzy`]).
//! 3. Una entrada aparece si coincidió al menos la mitad de las palabras (una
//!    pregunta trae verbos como "cambio" o "pongo" que no están en ningún
//!    título); cada palabra que no coincide resta puntaje.

use std::sync::LazyLock;

use liquidcontrol_core::tweaks;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{App, AppWindow, SearchResult, nav};

pub struct Entry {
    pane: String,
    /// Subpágina del panel ("" = la página principal).
    sub: String,
    /// Opción a resaltar al llegar ("" = el panel entero).
    item: String,
    title: String,
    /// Sección del catálogo, para ubicar el resultado ("Telemetría y diagnóstico").
    section: String,
    /// Sinónimos y palabras clave.
    keywords: String,
    description: String,
}

const MAX_RESULTS: usize = 14;

/// Panel (y subpágina) donde vive cada categoría del catálogo de ajustes.
const CATEGORY_PANES: [(&str, &str, &str); 6] = [
    ("privacy", "privacy", ""),
    ("desktop", "desktop", ""),
    ("optimize", "optimize", ""),
    ("accessibility", "accessibility", ""),
    ("gaming", "gaming", ""),
    ("updates", "general", "updates"),
];

/// Palabras que no aportan nada al buscar: se descartan para que una pregunta
/// entera ("¿dónde cambio el fondo de pantalla?") funcione igual que "fondo".
const STOPWORDS: [&str; 34] = [
    "a", "al", "como", "con", "cual", "cuales", "de", "del", "donde", "e", "el", "en", "esta", "este", "hay", "la",
    "las", "le", "lo", "los", "me", "mi", "o", "para", "pero", "por", "que", "se", "si", "su", "un", "una", "y", "yo",
];

/// Sinónimos y traducciones: cada palabra de la consulta también se busca como
/// sus alternativas, así "dark mode" encuentra "Apariencia".
const SYNONYMS: [(&str, &str); 63] = [
    ("rapido", "rendimiento optimizacion"),
    ("lento", "rendimiento optimizacion"),
    ("acelerar", "rendimiento optimizacion"),
    ("anuncios", "publicidad"),
    ("publicidad", "anuncios"),
    ("espacio", "almacenamiento disco"),
    ("limpiar", "limpieza optimizacion"),
    ("basura", "preinstaladas limpieza"),
    ("apagar", "desactivar"),
    ("prender", "activar"),
    ("sacar", "desactivar quitar ocultar"),
    ("quitar", "desactivar ocultar desinstalar"),
    ("borrar", "desinstalar quitar"),
    ("celular", "iphone airdrop"),
    ("wallpapers", "fondos"),
    ("dark", "oscuro"),
    ("light", "claro"),
    ("theme", "tema apariencia"),
    ("mode", "modo"),
    ("wallpaper", "fondo pantalla"),
    ("background", "fondo"),
    ("brightness", "brillo"),
    ("volume", "volumen"),
    ("sound", "sonido"),
    ("mic", "microfono"),
    ("microphone", "microfono"),
    ("camera", "camara"),
    ("battery", "bateria energia"),
    ("power", "energia"),
    ("sleep", "suspension"),
    ("update", "actualizaciones"),
    ("updates", "actualizaciones"),
    ("privacy", "privacidad"),
    ("telemetry", "telemetria"),
    ("ads", "publicidad anuncios"),
    ("startup", "inicio"),
    ("uninstall", "desinstalar"),
    ("install", "instalar"),
    ("apps", "aplicaciones"),
    ("network", "red"),
    ("keyboard", "teclado"),
    ("mouse", "raton puntero"),
    ("language", "idioma"),
    ("screen", "pantalla"),
    ("display", "pantalla"),
    ("resolution", "resolucion"),
    ("night", "nocturna"),
    ("firewall", "cortafuegos antivirus"),
    ("defender", "antivirus seguridad"),
    ("virus", "antivirus"),
    ("game", "juegos"),
    ("games", "juegos"),
    ("gaming", "juegos"),
    ("performance", "rendimiento optimizacion"),
    ("storage", "almacenamiento disco"),
    ("memory", "memoria ram"),
    ("clean", "limpiar limpieza"),
    ("bloatware", "preinstaladas"),
    ("taskbar", "barra tareas"),
    ("share", "compartir airdrop"),
    ("files", "archivos"),
    ("photos", "fotos"),
    ("password", "contrasena"),
];

/// Paneles y opciones sueltas (los ajustes del catálogo se suman aparte).
const BASE: &[(&str, &str, &str, &str)] = &[
    ("account", "", "Cuenta de Windows", "usuario perfil foto cuenta microsoft inicio de sesión pin windows hello"),
    ("wifi", "", "Wi‑Fi", "red inalámbrica internet conexión contraseña ssid wireless"),
    (
        "airdrop",
        "",
        "AirDrop",
        "compartir enviar archivos localsend transferir cerca red local mandar fotos celular nearby share",
    ),
    (
        "airdrop",
        "phone",
        "iPhone y celulares sin app",
        "qr codigo escanear camara iphone android celular safari pasar fotos",
    ),
    ("bluetooth", "", "Bluetooth", "auriculares dispositivos emparejar parlante headphones"),
    ("network", "", "Red", "ethernet ip dns proxy vpn adaptador cable"),
    ("notifications", "", "Notificaciones", "no molestar concentración avisos"),
    ("sound", "", "Sonido", "volumen altavoces parlantes micrófono audio salida entrada"),
    ("sound", "startup-sound", "Sonido de inicio", "arranque sonido al iniciar windows"),
    ("sound", "mixer", "Volumen por app", "mezclador aplicaciones sonido individual bajar volumen"),
    ("general", "", "General", "sistema acerca de pc equipo"),
    (
        "general",
        "about",
        "Información",
        "acerca de especificaciones procesador memoria ram cpu gpu gráficos versión windows nombre del equipo",
    ),
    ("general", "storage", "Almacenamiento", "disco espacio libre unidades ssd"),
    ("general", "startup", "Ítems de inicio", "arranque programas al iniciar sesión startup"),
    (
        "general",
        "updates",
        "Actualizaciones de Windows",
        "windows update buscar actualizaciones posponer pausar parches",
    ),
    ("appearance", "", "Apariencia", "modo oscuro claro mixto tema colores"),
    ("appearance", "accent", "Color de acento", "acento color windows barra de tareas títulos"),
    (
        "wallpaper",
        "",
        "Fondo de pantalla",
        "imagen wallpaper color sólido escritorio fondo fondos gratis wallhaven papel tapiz",
    ),
    ("appearance", "transparency", "Efectos de transparencia", "translúcido mica acrílico"),
    ("appearance", "animations", "Efectos de animación", "animaciones movimiento"),
    ("accessibility", "", "Accesibilidad", "lupa narrador contraste tamaño del texto"),
    ("desktop", "", "Escritorio y barra de tareas", "barra de tareas inicio widgets explorador"),
    ("displays", "", "Pantallas", "monitor resolución brillo escala hz frecuencia luz nocturna"),
    ("displays", "hdr", "HDR", "alto rango dinámico color brillo monitor"),
    ("energy", "", "Energía", "batería suspensión plan de energía apagado rendimiento"),
    ("energy", "plans", "Planes de energía", "crear borrar eliminar renombrar plan máximo rendimiento"),
    ("privacy", "", "Privacidad y seguridad", "telemetría ubicación cámara permisos"),
    (
        "security",
        "",
        "Antivirus",
        "seguridad defender firewall virus malware analizar escanear amenaza cuarentena protección tiempo real",
    ),
    ("optimize", "", "Optimización", "rendimiento tweaks limpieza bloatware servicios"),
    ("apps", "", "Aplicaciones", "programas desinstalar desinstalacion profunda restos winget instalar"),
    ("gaming", "", "Juegos", "juego gpu game bar modo de juego grabacion rendimiento fps"),
    ("mouse", "", "Mouse", "ratón puntero velocidad cursor desplazamiento doble clic"),
    ("mouse", "natural-scroll", "Desplazamiento natural", "rueda invertir dirección scroll macos"),
    ("keyboard", "", "Teclado", "repetición cursor"),
    (
        "keyboard",
        "languages",
        "Idiomas y distribuciones de teclado",
        "idioma distribucion layout latinoamericano estados unidos internacional escritura",
    ),
    ("account", "icloud", "iCloud", "apple drive fotos iphone contraseñas"),
    ("app", "", "Ajustes de la app", "preferencias tema portable acerca de"),
    ("app", "zoom", "Tamaño de la interfaz", "zoom texto grande agrandar letra escala"),
    ("app", "glass", "Liquid Glass y animaciones", "vidrio efectos animaciones movimiento transparencia"),
    ("app", "console", "Consola de actividad", "registro logs historial comandos salida output terminal"),
];

/// Índice completo: los paneles y opciones de arriba más cada ajuste del
/// catálogo, para que "copilot" o "widgets" lleven justo a su interruptor.
static INDEX: LazyLock<Vec<Entry>> = LazyLock::new(build_index);

fn build_index() -> Vec<Entry> {
    let mut index: Vec<Entry> = BASE
        .iter()
        .map(|(pane, item, title, keywords)| Entry {
            pane: (*pane).to_owned(),
            sub: String::new(),
            item: (*item).to_owned(),
            title: (*title).to_owned(),
            section: String::new(),
            keywords: (*keywords).to_owned(),
            description: String::new(),
        })
        .collect();
    for tweak in tweaks::index() {
        let Some((_, pane, sub)) = CATEGORY_PANES.iter().find(|(name, ..)| *name == tweak.category) else { continue };
        index.push(Entry {
            pane: (*pane).to_owned(),
            sub: (*sub).to_owned(),
            item: tweak.id,
            title: tweak.title,
            section: tweak.section,
            keywords: String::new(),
            description: tweak.description,
        });
    }
    index
}

pub fn init(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<App>().on_search_changed(move |query| {
        let Some(app) = weak.upgrade() else { return };
        let results = search(&app, &query);
        app.global::<App>().set_search_results(ModelRc::new(VecModel::from(results)));
    });
}

/// Resultados del buscador para una consulta en lenguaje natural.
pub fn search(app: &AppWindow, query: &str) -> Vec<SearchResult> {
    ranked(query).into_iter().filter_map(|entry| result(app, entry)).take(MAX_RESULTS).collect()
}

fn result(app: &AppWindow, entry: &Entry) -> Option<SearchResult> {
    let pane = nav::pane_info(app, &entry.pane)?;
    let context = if !entry.section.is_empty() {
        format!("{} · {}", pane.title, entry.section).into()
    } else if entry.item.is_empty() {
        SharedString::new()
    } else {
        pane.title
    };
    Some(SearchResult {
        pane: entry.pane.as_str().into(),
        // Con subpágina, el destino es la subpágina; si no, la opción a resaltar.
        item: if entry.sub.is_empty() { entry.item.as_str().into() } else { entry.sub.as_str().into() },
        title: entry.title.as_str().into(),
        context,
        icon: pane.icon,
        tint: pane.tint,
    })
}

/// Palabras de la consulta, ya normalizadas y sin relleno; cada una con sus
/// sinónimos como alternativas.
fn terms(query: &str) -> Vec<Vec<String>> {
    let all: Vec<String> =
        query.split(|c: char| !c.is_alphanumeric()).map(normalize).filter(|word| !word.is_empty()).collect();
    let useful: Vec<String> = all.iter().filter(|word| !STOPWORDS.contains(&word.as_str())).cloned().collect();
    // Si la consulta era toda relleno ("¿cómo?"), se usa igual lo que haya.
    let words = if useful.is_empty() { all } else { useful };
    words
        .into_iter()
        .map(|word| {
            let mut forms = vec![word.clone()];
            if let Some((_, alt)) = SYNONYMS.iter().find(|(from, _)| *from == word) {
                forms.extend(alt.split(' ').map(normalize));
            }
            forms
        })
        .collect()
}

/// Entradas que coinciden, de la mejor a la peor.
fn ranked(query: &str) -> Vec<&'static Entry> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(i32, usize, &Entry)> = INDEX
        .iter()
        .enumerate()
        .filter_map(|(position, entry)| score(entry, &terms).map(|points| (-points, position, entry)))
        .collect();
    // Mejor puntaje primero; a igual puntaje, el orden del índice (los paneles
    // van antes que los ajustes sueltos).
    hits.sort_by_key(|(points, position, _)| (*points, *position));
    hits.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Puntaje de una entrada (más alto es mejor); `None` si alguna palabra de la
/// consulta no aparece en ningún lado.
fn score(entry: &Entry, terms: &[Vec<String>]) -> Option<i32> {
    let title = normalize(&entry.title);
    let section = normalize(&entry.section);
    let keywords = normalize(&entry.keywords);
    let description = normalize(&entry.description);
    let mut total = 0;
    let mut matched = 0;
    for forms in terms {
        let best = forms
            .iter()
            .map(|word| {
                field_score(&title, word, 100)
                    .max(field_score(&keywords, word, 60))
                    .max(field_score(&section, word, 50))
                    .max(field_score(&description, word, 40))
            })
            .max()
            .unwrap_or(0);
        if best == 0 {
            // Palabra suelta que no está en ningún lado: resta, no descarta.
            total -= 20;
        } else {
            matched += 1;
            total += best;
        }
    }
    // Alcanza con que una palabra coincida bien: en "quiero que la pc ande más
    // rápido", la única que importa es "rápido". Pero cada palabra suelta
    // resta, así que si casi nada coincide el puntaje queda en cero y se
    // descarta igual.
    if matched == 0 || total <= 0 {
        return None;
    }
    // La consulta entera dentro del título (frase exacta) manda arriba.
    let whole = terms.iter().filter_map(|forms| forms.first()).cloned().collect::<Vec<_>>().join(" ");
    if title.contains(&whole) {
        total += 120;
    }
    if title.starts_with(&whole) {
        total += 80;
    }
    // Entre dos títulos que coinciden igual, gana el más corto y directo.
    Some(total - i32::try_from(title.len()).unwrap_or(0) / 8)
}

/// Cuánto vale encontrar `word` en un campo: al principio de una palabra vale
/// todo, contenida en el medio menos, y con un error de tipeo, la mitad.
fn field_score(field: &str, word: &str, weight: i32) -> i32 {
    if field.is_empty() {
        return 0;
    }
    if field == word {
        return weight + 40;
    }
    if field.split(' ').any(|part| part == word) {
        return weight + 20;
    }
    if field.split(' ').any(|part| part.starts_with(word)) {
        return weight;
    }
    if field.contains(word) {
        return weight - 25;
    }
    // Plurales simples: "colores" → "color", "juegos" → "juego".
    let stem = word.strip_suffix("es").or_else(|| word.strip_suffix('s')).unwrap_or(word);
    if stem.len() >= 4 && field.split(' ').any(|part| part.starts_with(stem)) {
        return weight - 15;
    }
    if field.split(' ').any(|part| fuzzy(part, word)) {
        return weight / 2;
    }
    0
}

/// ¿Son la misma palabra con un error de tipeo? Tolera uno (o dos si es larga).
fn fuzzy(a: &str, b: &str) -> bool {
    let (long, short) = if a.chars().count() >= b.chars().count() { (a, b) } else { (b, a) };
    let (long_len, short_len) = (long.chars().count(), short.chars().count());
    if short_len < 5 {
        return false;
    }
    let allowed = if short_len >= 8 { 2 } else { 1 };
    if long_len - short_len > allowed {
        return false;
    }
    distance(long, short, allowed) <= allowed
}

/// Distancia de edición, cortando apenas supera `limit` (no hace falta el valor
/// exacto y así no se recorre de más).
fn distance(a: &str, b: &str, limit: usize) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        let mut best = row[0];
        for (j, cb) in b.iter().enumerate() {
            let insert = row[j + 1] + 1;
            let delete = row[j] + 1;
            let replace = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = insert.min(delete).min(replace);
            best = best.min(row[j + 1]);
        }
        if best > limit {
            return limit + 1;
        }
    }
    row[b.len()]
}

/// Minúsculas, sin tildes ni guiones: la base de todas las comparaciones de texto.
pub fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => Some('a'),
            'é' | 'è' | 'ë' | 'ê' => Some('e'),
            'í' | 'ì' | 'ï' | 'î' => Some('i'),
            'ó' | 'ò' | 'ö' | 'ô' => Some('o'),
            'ú' | 'ù' | 'ü' | 'û' => Some('u'),
            'ñ' => Some('n'),
            '-' | '\u{2011}' | '_' => None,
            c => Some(c),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{INDEX, fuzzy, normalize, ranked};

    fn find(query: &str) -> Vec<&'static str> {
        ranked(query).into_iter().map(|entry| entry.title.as_str()).collect()
    }

    fn first(query: &str) -> Option<&'static str> {
        find(query).first().copied()
    }

    #[test]
    fn ignores_accents_and_hyphens() {
        assert_eq!(normalize("Wi‑Fi Energía"), "wifi energia");
        assert_eq!(first("wifi"), Some("Wi‑Fi"));
        assert_eq!(first("energia"), Some("Energía"));
    }

    #[test]
    fn matches_keywords() {
        assert!(find("ram").contains(&"Información"));
        assert!(find("modo oscuro").contains(&"Apariencia"));
        assert!(find("zzzqqq").is_empty());
    }

    #[test]
    fn indexes_the_whole_catalog() {
        assert!(INDEX.len() > 100, "índice chico: {}", INDEX.len());
        assert!(INDEX.iter().any(|entry| entry.title.to_lowercase().contains("copilot")));
    }

    #[test]
    fn finds_single_settings_by_name() {
        let hits = find("copilot");
        assert!(hits.iter().any(|title| title.to_lowercase().contains("copilot")), "{hits:?}");
    }

    #[test]
    fn understands_whole_questions() {
        assert_eq!(first("¿dónde cambio el fondo de pantalla?"), Some("Fondo de pantalla"));
        assert_eq!(first("como pongo modo oscuro"), Some("Apariencia"));
    }

    #[test]
    fn understands_english_and_typos() {
        assert_eq!(first("dark mode"), Some("Apariencia"));
        assert_eq!(first("wallpaper"), Some("Fondo de pantalla"));
        assert_eq!(first("blutooth"), Some("Bluetooth"));
        assert_eq!(first("notificasiones"), Some("Notificaciones"));
    }

    #[test]
    fn typos_only_within_reason() {
        assert!(fuzzy("bluetooth", "blutooth"));
        assert!(!fuzzy("sonido", "teclado"));
        assert!(!fuzzy("red", "ram"));
    }
}

#[cfg(test)]
mod demo {
    use super::ranked;

    #[test]
    #[ignore = "muestra el ranking real; correr con --ignored --nocapture"]
    fn top_results() {
        for query in [
            "copilot",
            "widgets",
            "desactivar telemetria",
            "como saco los anuncios de windows",
            "quiero que la pc ande mas rapido",
            "pasar fotos del iphone",
            "blutooth",
            "dark mode",
            "no me anda el microfono",
            "borrar apps que vienen instaladas",
            "recall",
            "barra de tareas a la izquierda",
        ] {
            let hits = ranked(query);
            let top: Vec<&str> = hits.iter().take(3).map(|e| e.title.as_str()).collect();
            println!("{query:>42}  →  {top:?}");
        }
    }
}

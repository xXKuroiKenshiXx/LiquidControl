//! Íconos grises de las filas (como los subíconos de Ajustes del Sistema).
//! `NAMES` sigue exactamente el orden de `Glyphs.all` en `ui/widgets/glyphs.slint`.

use crate::search;

pub const NAMES: [&str; 40] = [
    "activity",
    "camera",
    "megaphone",
    "history",
    "bot",
    "eye-off",
    "file",
    "database",
    "folder",
    "app-window",
    "languages",
    "shield",
    "package",
    "cpu",
    "eraser",
    "app-store",
    "map-pin",
    "mic",
    "search",
    "sparkles",
    "bell",
    "keyboard",
    "lock",
    "clock",
    "refresh-cw",
    "globe",
    "panel-bottom",
    "layout-grid",
    "monitor",
    "zap",
    "gamepad-2",
    "power",
    "hand",
    "gauge",
    "settings",
    "person-standing",
    "contrast",
    "wifi",
    "users",
    "mouse-pointer-2",
];

/// Palabras del título o la sección (sin tildes) → ícono. Gana la primera que coincida.
const RULES: &[(&[&str], &str)] = &[
    (&["ubicacion"], "map-pin"),
    (&["camara"], "camera"),
    (&["microfono", "voz"], "mic"),
    (&["copilot", "recall", "ia de", "inteligencia"], "sparkles"),
    (&["publicidad", "anuncio", "sugerencia", "promocion", "consejo"], "megaphone"),
    (&["historial", "actividad"], "history"),
    (&["busqueda", "bing", "buscar"], "search"),
    (&["telemetria", "diagnostico"], "activity"),
    (&["tarea programada", "tareas"], "clock"),
    (&["notificacion"], "bell"),
    (&["teclado", "escritura", "atajo", "tecla"], "keyboard"),
    (&["explorador", "archivo", "extension", "carpeta"], "folder"),
    (&["barra de tareas"], "panel-bottom"),
    (&["menu inicio", "widget"], "layout-grid"),
    (&["ventana", "multitarea", "ajustar"], "app-window"),
    (&["edge", "navegador"], "globe"),
    (&["actualiz", "update"], "refresh-cw"),
    (&["juego", "gpu", "grabacion"], "gamepad-2"),
    (&["energia", "suspension", "hibern"], "zap"),
    (&["defender", "seguridad", "smartscreen"], "shield"),
    (&["apps", "aplicacion", "tercero"], "app-store"),
    (&["red", "wifi", "internet"], "wifi"),
    (&["rendimiento", "prioridad", "procesador", "memoria"], "cpu"),
    (&["animacion", "efecto", "visual", "transparencia"], "sparkles"),
    (&["contraste", "color"], "contrast"),
    (&["cuenta", "usuario"], "users"),
    (&["mouse", "puntero"], "mouse-pointer-2"),
];

/// Posición del ícono en `Glyphs.all` (-1 si no existe).
pub fn index(name: &str) -> i32 {
    NAMES.iter().position(|known| *known == name).map_or(-1, |index| index as i32)
}

/// Ícono de un ajuste del catálogo: el que pida el catálogo o uno según su título y sección.
pub fn for_tweak(explicit: Option<&str>, title: &str, section: &str, category: &str) -> i32 {
    if let Some(found) = explicit.map(index).filter(|index| *index >= 0) {
        return found;
    }
    // Juegos: solo cuatro ajustes, todos con el mismo tema; un glifo por fila no
    // agrega nada (y siempre saldría el mismo ícono). Van sin ícono.
    if category == "gaming" {
        return -1;
    }
    let text = search::normalize(&format!("{title} {section}"));
    RULES
        .iter()
        .find(|(words, _)| words.iter().any(|word| text.contains(word)))
        .map(|(_, icon)| index(icon))
        .unwrap_or_else(|| {
            index(match category {
                "privacy" => "hand",
                "desktop" => "panel-bottom",
                "accessibility" => "person-standing",
                _ => "gauge",
            })
        })
}

#[cfg(test)]
mod tests {
    use super::{NAMES, RULES, for_tweak, index};

    #[test]
    fn every_rule_points_to_a_known_icon() {
        assert!(RULES.iter().all(|(_, icon)| index(icon) >= 0));
        assert_eq!(index(NAMES[0]), 0);
        assert_eq!(index("no-existe"), -1);
    }

    #[test]
    fn picks_icons_by_meaning() {
        assert_eq!(for_tweak(None, "Acceso a la cámara", "Permisos de apps", "privacy"), index("camera"));
        assert_eq!(for_tweak(None, "Algo raro", "Otra cosa", "privacy"), index("hand"));
        assert_eq!(for_tweak(Some("lock"), "Acceso a la cámara", "", "privacy"), index("lock"));
    }

    #[test]
    fn gaming_tweaks_have_no_icon() {
        assert_eq!(for_tweak(None, "Modo de juego", "Rendimiento en juegos", "gaming"), -1);
    }
}

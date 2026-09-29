//! Formato de tamaños y duraciones para mostrar en la interfaz (es-AR).

/// Tamaño con unidades binarias, igual que el Explorador: "15,9 GB".
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} bytes")
    } else if size >= 100.0 {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// Duración aproximada: "2 d 1 h", "5 h 7 min", "12 min".
pub fn duration(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes} min"),
        (0, _) => format!("{hours} h {minutes} min"),
        _ => format!("{days} d {hours} h"),
    }
}

/// Inicial para íconos sin imagen: "Spotify" → "S", "7-Zip" → "7".
pub fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

/// Índice de color estable (0–7, ver `Tints.palette`) para íconos genéricos:
/// la misma app siempre tiene el mismo color, entre recargas y entre sesiones.
pub fn tint_index(text: &str) -> i32 {
    const PALETTE_LEN: u32 = 8;
    let hash = text.bytes().fold(0u32, |acc, byte| acc.wrapping_mul(31).wrapping_add(u32::from(byte)));
    (hash % PALETTE_LEN) as i32
}

#[cfg(test)]
mod tests {
    use super::{bytes, duration, initial, tint_index};

    #[test]
    fn initials() {
        assert_eq!(initial("spotify"), "S");
        assert_eq!(initial("7-Zip"), "7");
        assert_eq!(initial("  "), "");
    }

    #[test]
    fn stable_tints() {
        assert_eq!(tint_index("Microsoft.BingNews"), tint_index("Microsoft.BingNews"));
        assert!((0..8).contains(&tint_index("Microsoft.BingNews")));
        assert_eq!(tint_index(""), 0);
    }

    #[test]
    fn sizes() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(1536), "1,5 KB");
        assert_eq!(bytes(512 * 1024 * 1024 * 1024), "512 GB");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(59), "0 min");
        assert_eq!(duration(5 * 3_600 + 7 * 60), "5 h 7 min");
        assert_eq!(duration(2 * 86_400 + 3_600), "2 d 1 h");
    }
}

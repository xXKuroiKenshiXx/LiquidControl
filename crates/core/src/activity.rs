//! Registro de actividad: cada acción que cambia el sistema (registro, servicios,
//! tareas, procesos, planes de energía…) queda anotada con hora, área y resultado.
//! La interfaz lo muestra como una consola integrada y además se guarda en
//! `logs\liquidcontrol.log` dentro de la carpeta de datos.
//!
//! Nunca se registran contraseñas ni otros datos secretos.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::paths;

/// Entradas que se guardan en memoria para la consola.
const MAX_ENTRIES: usize = 500;
/// Al pasar este tamaño, el archivo actual pasa a `liquidcontrol.old.log`.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Success => "OK",
            Self::Warning => "AVISO",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Hora local "HH:MM:SS".
    pub time: String,
    pub level: Level,
    /// Área: "registro", "servicios", "wifi", "energía"…
    pub area: String,
    pub message: String,
}

type Listener = Arc<dyn Fn(&Entry) + Send + Sync>;

#[derive(Default)]
struct State {
    entries: VecDeque<Entry>,
    listener: Option<Listener>,
    file: Option<PathBuf>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Prepara el archivo de log e instala el registro como destino del crate `log`,
/// así los `log::info!` de todo el programa también llegan a la consola.
pub fn init() {
    if let Ok(dir) = paths::data_dir().map(|dir| dir.join("logs")) {
        if fs::create_dir_all(&dir).is_ok() {
            let file = dir.join("liquidcontrol.log");
            if fs::metadata(&file).is_ok_and(|meta| meta.len() > MAX_FILE_BYTES) {
                let _ = fs::rename(&file, dir.join("liquidcontrol.old.log"));
            }
            lock().file = Some(file);
        }
    }
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// Anota una entrada.
pub fn record(level: Level, area: &str, message: impl Into<String>) {
    let (date, time) = now();
    let entry = Entry { time, level, area: area.to_owned(), message: message.into() };
    let listener = {
        let mut state = lock();
        if let Some(path) = &state.file {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ =
                    writeln!(file, "{date} {} [{}] {}: {}", entry.time, entry.level.tag(), entry.area, entry.message);
            }
        }
        state.entries.push_back(entry.clone());
        while state.entries.len() > MAX_ENTRIES {
            state.entries.pop_front();
        }
        state.listener.clone()
    };
    // Fuera del candado: el oyente puede tardar o volver a registrar.
    if let Some(listener) = listener {
        listener(&entry);
    }
}

pub fn info(area: &str, message: impl Into<String>) {
    record(Level::Info, area, message);
}

pub fn success(area: &str, message: impl Into<String>) {
    record(Level::Success, area, message);
}

pub fn warning(area: &str, message: impl Into<String>) {
    record(Level::Warning, area, message);
}

pub fn error(area: &str, message: impl Into<String>) {
    record(Level::Error, area, message);
}

/// Copia de las entradas en memoria (de la más vieja a la más nueva).
pub fn entries() -> Vec<Entry> {
    lock().entries.iter().cloned().collect()
}

/// Vacía la consola (el archivo de log se conserva).
pub fn clear() {
    lock().entries.clear();
}

/// Se llama con cada entrada nueva, desde el hilo que la registró.
pub fn set_listener(listener: impl Fn(&Entry) + Send + Sync + 'static) {
    lock().listener = Some(Arc::new(listener));
}

/// Ruta del archivo de log actual.
pub fn log_file() -> Option<PathBuf> {
    lock().file.clone()
}

/// ("AAAA-MM-DD", "HH:MM:SS") en hora local.
fn now() -> (String, String) {
    let time = unsafe { GetLocalTime() };
    (
        format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay),
        format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond),
    )
}

/// Área legible a partir del destino de `log`: "liquidcontrol_core::tweaks" → "tweaks".
/// Solo se aceptan mensajes propios (no los de dependencias como Slint o winit).
fn area_for(target: &str) -> Option<&str> {
    if !target.contains("::") {
        return Some(target);
    }
    if target.starts_with("liquidcontrol") {
        return target.rsplit("::").next();
    }
    None
}

struct Logger;

static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info && area_for(metadata.target()).is_some()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => Level::Error,
            log::Level::Warn => Level::Warning,
            _ => Level::Info,
        };
        let area = area_for(record.target()).unwrap_or("app");
        self::record(level, area, record.args().to_string());
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::area_for;

    #[test]
    fn areas_from_targets() {
        assert_eq!(area_for("registro"), Some("registro"));
        assert_eq!(area_for("liquidcontrol_core::tweaks"), Some("tweaks"));
        assert_eq!(area_for("liquidcontrol::panes::wifi"), Some("wifi"));
        assert_eq!(area_for("i_slint_core::window"), None);
    }
}

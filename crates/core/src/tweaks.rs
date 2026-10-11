//! Motor declarativo de ajustes reversibles para Windows.
//!
//! Cada ajuste de `catalog/tweaks.json` (embebido en el binario) es una lista de
//! operaciones —valor de registro, clave, servicio o tarea programada— con su
//! estado "activado". El motor:
//!
//! 1. **Lee el estado real** del sistema: activo, inactivo o aplicado a medias.
//! 2. **Guarda el valor anterior** de cada operación en un diario antes de aplicar.
//! 3. **Revierte exactamente** a lo que había; si no hay diario, usa los valores
//!    `off` del catálogo.
//!
//! La interfaz solo envía el id del ajuste: nunca rutas ni valores.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::paths::{data_dir, strip_bom, write_atomic};
use crate::util::services::{self, START_AUTO, START_DISABLED, START_MANUAL};
use crate::util::win::{broadcast_setting_change, is_elevated};
use crate::util::{com, reg, sysapp, tasks};

const CATALOG_JSON: &str = include_str!("../catalog/tweaks.json");

/// Serializa las escrituras del motor (y del diario) entre hilos.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

// ─────────────────────────── Catálogo ───────────────────────────

#[derive(Deserialize)]
struct Catalog {
    tweaks: Vec<TweakDef>,
}

#[derive(Deserialize, Clone)]
struct TweakDef {
    id: String,
    category: String,
    section: String,
    title: String,
    description: String,
    risk: Risk,
    restart: Restart,
    /// Solo se muestra si existe esta clave (p. ej. la app de terceros está instalada).
    #[serde(default)]
    when: Option<String>,
    /// Ícono gris de la fila (nombre de `crates/app/ui/icons`); si falta, la app elige uno.
    #[serde(default)]
    icon: Option<String>,
    ops: Vec<Op>,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Moderate,
    Advanced,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Restart {
    None,
    Explorer,
    Logoff,
    Reboot,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Dword,
    Qword,
    Sz,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum StartType {
    Auto,
    Manual,
    Disabled,
}

impl StartType {
    fn code(self) -> u32 {
        match self {
            Self::Auto => START_AUTO,
            Self::Manual => START_MANUAL,
            Self::Disabled => START_DISABLED,
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum TaskState {
    Enabled,
    Disabled,
}

/// Operación del catálogo. Se distingue por el campo presente (`reg`, `regKey`, `service`, `task`).
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum Op {
    RegValue {
        reg: String,
        name: String,
        #[serde(rename = "type")]
        kind: Kind,
        on: Value,
        /// `null` o ausente = borrar el valor al revertir.
        #[serde(default)]
        off: Option<Value>,
    },
    RegKey {
        #[serde(rename = "regKey")]
        key: String,
        on: bool,
    },
    Service {
        service: String,
        on: StartType,
        off: StartType,
    },
    Task {
        task: String,
        on: TaskState,
    },
    /// App del sistema que se desactiva renombrando su carpeta de `SystemApps`:
    /// "activado" = desactivada. Se la identifica por su ejecutable y la carpeta
    /// se busca en cada PC (su nombre cambia entre versiones de Windows).
    SystemApp {
        #[serde(rename = "systemApp")]
        exe: String,
    },
}

fn catalog() -> AppResult<&'static Catalog> {
    static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(CATALOG_JSON).map_err(|err| err.to_string()))
        .as_ref()
        .map_err(|err| AppError::failed(format!("Catálogo de ajustes inválido: {err}")))
}

// ─────────────────────────── Valores de registro ───────────────────────────

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
enum RegData {
    Dword(u32),
    Qword(u64),
    Sz(String),
}

impl RegData {
    fn from_json(kind: Kind, value: &Value) -> AppResult<Self> {
        let data = match kind {
            Kind::Dword => value.as_u64().and_then(|v| u32::try_from(v).ok()).map(Self::Dword),
            Kind::Qword => value.as_u64().map(Self::Qword),
            Kind::Sz => value.as_str().map(|s| Self::Sz(s.to_owned())),
        };
        data.ok_or_else(|| AppError::failed(format!("Valor de catálogo inválido: {value}")))
    }
}

fn read_reg(path: &str, name: &str, kind: Kind) -> Option<RegData> {
    match kind {
        Kind::Dword => reg::read_dword(path, name).map(RegData::Dword),
        Kind::Qword => reg::read_qword(path, name).map(RegData::Qword),
        Kind::Sz => reg::read_string(path, name).map(RegData::Sz),
    }
}

fn write_reg(path: &str, name: &str, data: &RegData) -> AppResult<()> {
    match data {
        RegData::Dword(v) => reg::write_dword(path, name, *v),
        RegData::Qword(v) => reg::write_qword(path, name, *v),
        RegData::Sz(v) => reg::write_string(path, name, v),
    }
}

// ─────────────────────────── Diario (valores previos) ───────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Previous {
    Value { path: String, name: String, data: Option<RegData> },
    Key { path: String, existed: bool },
    Service { name: String, start: u32 },
    Task { path: String, enabled: bool },
    SystemApp { exe: String, disabled: bool },
}

type Journal = HashMap<String, Vec<Previous>>;

fn load_journal(path: &Path) -> Journal {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(strip_bom(&text)).ok()).unwrap_or_default()
}

fn save_journal(path: &Path, journal: &Journal) -> AppResult<()> {
    write_atomic(path, &serde_json::to_string_pretty(journal)?)
}

// ─────────────────────────── Estado ───────────────────────────

/// `Some(true)` si la operación está en su valor "activado", `None` si no aplica
/// en este equipo (servicio o tarea inexistente).
fn op_state(op: &Op) -> Option<bool> {
    match op {
        Op::RegValue { reg: path, name, kind, on, .. } => {
            let target = RegData::from_json(*kind, on).ok()?;
            Some(read_reg(path, name, *kind).as_ref() == Some(&target))
        }
        Op::RegKey { key, on } => Some(reg::key_exists(key) == *on),
        Op::Service { service, on, .. } => services::start_type(service).map(|start| start == on.code()),
        Op::Task { task, on } => tasks::enabled(task).map(|enabled| enabled == (*on == TaskState::Enabled)),
        Op::SystemApp { exe } => sysapp::is_disabled(exe),
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum TweakState {
    On,
    Off,
    Partial,
}

fn tweak_state(def: &TweakDef) -> TweakState {
    let states: Vec<bool> = def.ops.iter().filter_map(op_state).collect();
    match (states.iter().filter(|s| **s).count(), states.len()) {
        (_, 0) | (0, _) => TweakState::Off,
        (on, total) if on == total => TweakState::On,
        _ => TweakState::Partial,
    }
}

fn needs_admin(def: &TweakDef) -> bool {
    def.ops.iter().any(|op| match op {
        Op::RegValue { reg: path, .. } => reg::is_machine(path),
        Op::RegKey { key, .. } => reg::is_machine(key),
        Op::Service { .. } | Op::Task { .. } | Op::SystemApp { .. } => true,
    })
}

/// Vista de un ajuste para la interfaz.
#[derive(Debug, Clone, Serialize)]
pub struct Tweak {
    pub id: String,
    pub category: String,
    pub section: String,
    pub title: String,
    pub description: String,
    pub risk: Risk,
    pub admin: bool,
    pub restart: Restart,
    pub state: TweakState,
    /// Qué hace exactamente, una línea por operación (registro, servicio, tarea).
    pub details: Vec<String>,
    /// Ícono pedido por el catálogo, si hay.
    pub icon: Option<String>,
}

fn show_value(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => other.to_string(),
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Dword => "DWORD",
        Kind::Qword => "QWORD",
        Kind::Sz => "texto",
    }
}

/// Descripción de una operación del catálogo, para "Ver qué hace".
fn describe(op: &Op) -> String {
    match op {
        Op::RegValue { reg: path, name, kind, on, off } => {
            let revert = off.as_ref().map_or_else(|| "se borra".to_owned(), show_value);
            format!(
                "Registro {path}\\{name} = {} ({}). Al revertir: el valor anterior (o {revert}).",
                show_value(on),
                kind_name(*kind)
            )
        }
        Op::RegKey { key, on: true } => format!("Crea la clave {key}."),
        Op::RegKey { key, on: false } => format!("Borra la clave {key} (se guarda lo que había)."),
        Op::Service { service, on, off } => format!(
            "Servicio {service}: inicio {}. Al revertir: {}.",
            services::start_name(on.code()),
            services::start_name(off.code())
        ),
        Op::Task { task, on } => {
            format!(
                "Tarea programada {task}: {}.",
                if *on == TaskState::Enabled { "habilitada" } else { "deshabilitada" }
            )
        }
        Op::SystemApp { exe } => format!(
            "Busca en SystemApps (dentro de la carpeta de Windows) la carpeta que trae {exe}, cierra el programa y \
             la renombra agregándole «.old», así Windows no lo ejecuta. Al revertir: vuelve a su nombre y a su \
             propietario original."
        ),
    }
}

fn to_view(def: &TweakDef) -> Tweak {
    Tweak {
        id: def.id.clone(),
        category: def.category.clone(),
        section: def.section.clone(),
        title: def.title.clone(),
        description: def.description.clone(),
        risk: def.risk,
        admin: needs_admin(def),
        restart: def.restart,
        state: tweak_state(def),
        details: def.ops.iter().map(describe).collect(),
        icon: def.icon.clone(),
    }
}

// ─────────────────────────── Aplicar / revertir ───────────────────────────

fn capture(op: &Op) -> Option<Previous> {
    match op {
        Op::RegValue { reg: path, name, kind, .. } => {
            Some(Previous::Value { path: path.clone(), name: name.clone(), data: read_reg(path, name, *kind) })
        }
        Op::RegKey { key, .. } => Some(Previous::Key { path: key.clone(), existed: reg::key_exists(key) }),
        Op::Service { service, .. } => {
            services::start_type(service).map(|start| Previous::Service { name: service.clone(), start })
        }
        Op::Task { task, .. } => tasks::enabled(task).map(|enabled| Previous::Task { path: task.clone(), enabled }),
        Op::SystemApp { exe } => {
            sysapp::is_disabled(exe).map(|disabled| Previous::SystemApp { exe: exe.clone(), disabled })
        }
    }
}

fn apply_op(op: &Op) -> AppResult<()> {
    match op {
        Op::RegValue { reg: path, name, kind, on, .. } => write_reg(path, name, &RegData::from_json(*kind, on)?),
        Op::RegKey { key, on: true } => reg::create_key(key),
        Op::RegKey { key, on: false } => reg::delete_tree(key),
        Op::Service { service, on, .. } => services::set_start_type(service, on.code()),
        Op::Task { task, on } => tasks::set_enabled(task, *on == TaskState::Enabled),
        Op::SystemApp { exe } => sysapp::disable(exe),
    }
}

fn revert_op(op: &Op) -> AppResult<()> {
    match op {
        Op::RegValue { reg: path, name, kind, off: Some(value), .. } => {
            write_reg(path, name, &RegData::from_json(*kind, value)?)
        }
        Op::RegValue { reg: path, name, off: None, .. } => reg::delete_value(path, name),
        Op::RegKey { key, on: true } => reg::delete_tree(key),
        Op::RegKey { key, on: false } => reg::create_key(key),
        Op::Service { service, off, .. } => services::set_start_type(service, off.code()),
        Op::Task { task, on } => tasks::set_enabled(task, *on != TaskState::Enabled),
        Op::SystemApp { exe } => sysapp::enable(exe),
    }
}

fn restore(previous: &Previous) -> AppResult<()> {
    match previous {
        Previous::Value { path, name, data: Some(data) } => write_reg(path, name, data),
        Previous::Value { path, name, data: None } => reg::delete_value(path, name),
        Previous::Key { path, existed: true } => reg::create_key(path),
        Previous::Key { path, existed: false } => reg::delete_tree(path),
        Previous::Service { name, start } => services::set_start_type(name, *start),
        Previous::Task { path, enabled } => tasks::set_enabled(path, *enabled),
        Previous::SystemApp { exe, disabled: true } => sysapp::disable(exe),
        Previous::SystemApp { exe, disabled: false } => sysapp::enable(exe),
    }
}

fn apply(def: &TweakDef, journal: &mut Journal) -> AppResult<()> {
    // Si ya había un registro previo se conserva el original (el de antes de la primera vez).
    journal.entry(def.id.clone()).or_insert_with(|| def.ops.iter().filter_map(capture).collect());
    for op in &def.ops {
        // Servicios o tareas que no existen en esta edición de Windows se omiten.
        if op_state(op).is_some() {
            apply_op(op)?;
        }
    }
    Ok(())
}

fn revert_defaults(def: &TweakDef) -> AppResult<()> {
    def.ops.iter().rev().filter(|op| op_state(op).is_some()).try_for_each(revert_op)
}

/// ¿La entrada del diario corresponde a una operación de este ajuste (misma
/// clave, servicio o tarea, y mismo tipo de dato)? El diario vive en una carpeta
/// del usuario: así, aunque alguien lo modifique, revertir nunca escribe fuera de
/// lo que el propio ajuste toca.
fn belongs_to(def: &TweakDef, previous: &Previous) -> bool {
    def.ops.iter().any(|op| match (op, previous) {
        (Op::RegValue { reg: path, name, kind, .. }, Previous::Value { path: p, name: n, data }) => {
            path.eq_ignore_ascii_case(p)
                && name.eq_ignore_ascii_case(n)
                && data.as_ref().is_none_or(|data| {
                    matches!(
                        (kind, data),
                        (Kind::Dword, RegData::Dword(_))
                            | (Kind::Qword, RegData::Qword(_))
                            | (Kind::Sz, RegData::Sz(_))
                    )
                })
        }
        (Op::RegKey { key, .. }, Previous::Key { path, .. }) => key.eq_ignore_ascii_case(path),
        (Op::Service { service, .. }, Previous::Service { name, start }) => {
            service.eq_ignore_ascii_case(name) && [START_AUTO, START_MANUAL, START_DISABLED].contains(start)
        }
        (Op::Task { task, .. }, Previous::Task { path, .. }) => task.eq_ignore_ascii_case(path),
        (Op::SystemApp { exe }, Previous::SystemApp { exe: previous_exe, .. }) => {
            exe.eq_ignore_ascii_case(previous_exe)
        }
        _ => false,
    })
}

fn revert(def: &TweakDef, journal: &mut Journal) -> AppResult<()> {
    let Some(previous) = journal.remove(&def.id) else { return revert_defaults(def) };
    let (valid, ignored): (Vec<Previous>, Vec<Previous>) =
        previous.into_iter().partition(|entry| belongs_to(def, entry));
    if !ignored.is_empty() {
        log::warn!(
            target: "tweaks",
            "Se ignoraron {} entradas del diario que no corresponden a «{}»",
            ignored.len(),
            def.title
        );
    }
    if valid.is_empty() {
        return revert_defaults(def);
    }
    valid.iter().rev().try_for_each(restore)
}

/// Avisa al sistema de los cambios para que se apliquen sin reiniciar cuando es posible.
fn notify(def: &TweakDef) {
    let mut areas = BTreeSet::new();
    for op in &def.ops {
        let path = match op {
            Op::RegValue { reg: path, .. } => path.as_str(),
            Op::RegKey { key, .. } => key.as_str(),
            _ => continue,
        };
        if path.ends_with(r"\Environment") {
            areas.insert("Environment");
        } else if path.contains("Policies") {
            areas.insert("Policy");
        } else if path.contains(r"\Explorer") || path.contains(r"\Search") {
            areas.insert("TraySettings");
        }
    }
    for area in &areas {
        broadcast_setting_change(Some(area));
    }
    broadcast_setting_change(None);
}

// ─────────────────────────── API pública ───────────────────────────

/// Una entrada del catálogo tal como está escrita, sin consultar el sistema:
/// alcanza para el buscador de la app, que tiene que ser instantáneo.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub category: String,
    pub section: String,
    pub title: String,
    pub description: String,
}

/// Todo el catálogo para indexarlo en el buscador (no toca el registro).
pub fn index() -> Vec<CatalogEntry> {
    catalog().map_or_else(
        |_| Vec::new(),
        |catalog| {
            catalog
                .tweaks
                .iter()
                .map(|def| CatalogEntry {
                    id: def.id.clone(),
                    category: def.category.clone(),
                    section: def.section.clone(),
                    title: def.title.clone(),
                    description: def.description.clone(),
                })
                .collect()
        },
    )
}

/// Ajustes de una categoría (`privacy`, `desktop`, `optimize`, `accessibility`) con su estado real.
pub fn list(category: &str) -> AppResult<Vec<Tweak>> {
    let category = category.to_owned();
    // Las tareas programadas se consultan por COM.
    com::mta(move || {
        Ok(catalog()?
            .tweaks
            .iter()
            .filter(|def| def.category == category)
            .filter(|def| def.when.as_deref().is_none_or(reg::key_exists))
            .map(to_view)
            .collect())
    })
}

/// Activa (`true`) o revierte (`false`) un ajuste y devuelve su nuevo estado.
pub fn set(id: &str, enabled: bool) -> AppResult<Tweak> {
    let journal_path = data_dir()?.join("journal.json");
    let id = id.to_owned();
    com::mta(move || {
        let def = catalog()?
            .tweaks
            .iter()
            .find(|def| def.id == id)
            .ok_or_else(|| AppError::not_found(format!("Ajuste desconocido: {id}")))?;
        if needs_admin(def) && !is_elevated() {
            return Err(AppError::NeedsAdmin);
        }
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut journal = load_journal(&journal_path);
        let result = if enabled { apply(def, &mut journal) } else { revert(def, &mut journal) };
        // El diario se guarda aunque falle a mitad de camino: así siempre se puede revertir.
        save_journal(&journal_path, &journal)?;
        result?;
        notify(def);
        log::info!(target: "tweaks", "«{}» {}", def.title, if enabled { "activado" } else { "revertido" });
        Ok(to_view(def))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_entries_must_belong_to_the_tweak() {
        let def = catalog().unwrap().tweaks.iter().find(|def| matches!(def.ops[0], Op::RegValue { .. })).unwrap();
        let Op::RegValue { reg: path, name, .. } = &def.ops[0] else { unreachable!() };
        let own = Previous::Value { path: path.clone(), name: name.clone(), data: None };
        let foreign = Previous::Value {
            path: r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run".into(),
            name: "evil".into(),
            data: Some(RegData::Sz("C:\\evil.exe".into())),
        };
        assert!(belongs_to(def, &own));
        assert!(!belongs_to(def, &foreign));
        assert!(!describe(&def.ops[0]).is_empty());
    }

    #[test]
    fn catalog_is_valid() {
        let catalog = catalog().expect("el catálogo debe parsear");
        let mut ids = BTreeSet::new();
        for def in &catalog.tweaks {
            assert!(ids.insert(def.id.as_str()), "id duplicado: {}", def.id);
            assert!(
                ["privacy", "desktop", "optimize", "gaming", "accessibility", "updates"]
                    .contains(&def.category.as_str()),
                "categoría desconocida en {}",
                def.id
            );
            assert!(!def.ops.is_empty(), "{} no tiene operaciones", def.id);
            for op in &def.ops {
                match op {
                    Op::RegValue { reg: path, kind, on, off, .. } => {
                        reg::split(path).expect("ruta de registro válida");
                        RegData::from_json(*kind, on).expect("valor on válido");
                        if let Some(off) = off {
                            RegData::from_json(*kind, off).expect("valor off válido");
                        }
                    }
                    Op::SystemApp { exe } => {
                        assert!(sysapp::valid_exe(exe), "ejecutable inválido en {}", def.id);
                    }
                    _ => {}
                }
            }
        }
    }
}

//! Tareas programadas vía la API COM del Programador de tareas.
//! Requiere COM inicializado en el hilo (ver `util::com`).

use windows::Win32::Foundation::{VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{IRegisteredTask, ITaskService, TaskScheduler};
use windows::Win32::System::Variant::VARIANT;
use windows::core::BSTR;

use crate::error::{AppError, AppResult};

fn connect() -> AppResult<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        Ok(service)
    }
}

fn find(path: &str) -> Option<IRegisteredTask> {
    let (folder, name) = path.rsplit_once('\\')?;
    let folder = if folder.is_empty() { "\\" } else { folder };
    unsafe { connect().ok()?.GetFolder(&BSTR::from(folder)).ok()?.GetTask(&BSTR::from(name)).ok() }
}

/// `Some(habilitada)` o `None` si la tarea no existe en este sistema.
pub fn enabled(path: &str) -> Option<bool> {
    let task = find(path)?;
    let value: VARIANT_BOOL = unsafe { task.Enabled().ok()? };
    Some(value.as_bool())
}

pub fn set_enabled(path: &str, on: bool) -> AppResult<()> {
    let task = find(path).ok_or_else(|| AppError::not_found(format!("No existe la tarea {path}")))?;
    unsafe { task.SetEnabled(if on { VARIANT_TRUE } else { VARIANT_FALSE })? };
    log::info!(target: "tareas", "Tarea {path}: {}", if on { "habilitada" } else { "deshabilitada" });
    Ok(())
}

//! Tipo de inicio de servicios de Windows (lectura por registro, escritura por SCM).

use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_TYPE, OpenSCManagerW, OpenServiceW,
    SC_MANAGER_CONNECT, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP, SERVICE_ERROR, SERVICE_NO_CHANGE,
    SERVICE_START_TYPE, SERVICE_STATUS, SERVICE_STOP,
};
use windows::core::{HSTRING, PCWSTR};

use crate::error::AppResult;
use crate::util::reg;

pub const START_AUTO: u32 = 2;
pub const START_MANUAL: u32 = 3;
pub const START_DISABLED: u32 = 4;

/// Nombre legible del tipo de inicio.
pub fn start_name(start: u32) -> &'static str {
    match start {
        START_AUTO => "automático",
        START_MANUAL => "manual",
        START_DISABLED => "deshabilitado",
        _ => "otro",
    }
}

/// Tipo de inicio actual (2 automático, 3 manual, 4 deshabilitado) o `None` si no existe.
pub fn start_type(name: &str) -> Option<u32> {
    reg::read_dword(&format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{name}"), "Start")
}

/// Cambia el tipo de inicio. Si se deshabilita, además intenta detenerlo.
pub fn set_start_type(name: &str, start: u32) -> AppResult<()> {
    let result = unsafe {
        let manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)?;
        let result = (|| -> AppResult<()> {
            let service = OpenServiceW(manager, &HSTRING::from(name), SERVICE_CHANGE_CONFIG | SERVICE_STOP)?;
            let changed = ChangeServiceConfigW(
                service,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(start),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            );
            if changed.is_ok() && start == START_DISABLED {
                let mut status = SERVICE_STATUS::default();
                let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
            }
            let _ = CloseServiceHandle(service);
            changed.map_err(Into::into)
        })();
        let _ = CloseServiceHandle(manager);
        result
    };
    if result.is_ok() {
        log::info!(target: "servicios", "Servicio {name}: inicio {}", start_name(start));
    }
    result
}

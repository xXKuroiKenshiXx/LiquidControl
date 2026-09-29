//! Helpers de Win32: cadenas anchas, avisos de cambio de configuración y elevación.

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};

/// Convierte un búfer UTF-16 terminado en nulo (o no) a `String`.
pub fn from_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

/// Avisa a todas las ventanas que cambió una configuración del sistema
/// (por ejemplo `"ImmersiveColorSet"` para el tema o `"Environment"`).
pub fn broadcast_setting_change(area: Option<&str>) {
    let text = area.map(HSTRING::from);
    let lparam = text.as_ref().map_or(0, |t| t.as_ptr() as isize);
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG,
            250,
            None,
        );
    }
}

/// ¿El proceso actual tiene un token elevado (administrador)?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

pub fn require_admin() -> AppResult<()> {
    if is_elevated() { Ok(()) } else { Err(AppError::NeedsAdmin) }
}

/// Carpeta de Windows (`C:\Windows`), para invocar herramientas del sistema por
/// ruta absoluta y no depender del PATH (evita secuestro de ejecutables).
pub fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

pub fn system32(exe: &str) -> PathBuf {
    system_root().join("System32").join(exe)
}

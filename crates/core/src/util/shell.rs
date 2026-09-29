//! Abrir URIs/carpetas con el shell, elevación y reinicio del Explorador.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, PostMessageW, SW_SHOWNORMAL};
use windows::core::{HRESULT, HSTRING, PCWSTR, w};

use crate::error::{AppError, AppResult};
use crate::util::win;

const ERROR_CANCELLED: u32 = 1223;
/// Mensaje privado que hace que el Explorador se cierre de forma ordenada.
const WM_EXIT_EXPLORER: u32 = 0x05B4;

/// Abre una URI, archivo o carpeta con su aplicación predeterminada.
/// Debe llamarse desde un hilo STA (ver `util::com::sta`).
pub fn open(target: &str) -> AppResult<()> {
    log::info!(target: "shell", "Abriendo {target}");
    let target = HSTRING::from(target);
    let result = unsafe { ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if result.0 as isize > 32 { Ok(()) } else { Err(AppError::failed("Windows no pudo abrir el destino.")) }
}

fn execute(verb: PCWSTR, file: &HSTRING, params: &str) -> AppResult<()> {
    let params = HSTRING::from(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: verb,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|err| {
        if err.code() == HRESULT::from_win32(ERROR_CANCELLED) {
            AppError::Coded("cancelled", "Se canceló la solicitud de permisos.".into())
        } else {
            err.into()
        }
    })
}

/// Lanza `exe` con el verbo "runas" (UAC).
pub fn run_as_admin(exe: &Path, params: &str) -> AppResult<()> {
    log::info!(target: "shell", "Pidiendo permisos de administrador para {} {params}", exe.display());
    execute(w!("runas"), &HSTRING::from(exe.as_os_str()), params)
}

/// Lanza un programa con argumentos a través del shell: si su manifiesto pide
/// administrador, Windows muestra el UAC. Debe llamarse desde un hilo STA.
pub fn launch(file: &str, params: &str) -> AppResult<()> {
    log::info!(target: "shell", "Ejecutando {file} {params}");
    execute(PCWSTR::null(), &HSTRING::from(file), params)
}

/// Reinicia el Explorador para aplicar cambios de la barra de tareas y del shell.
pub fn restart_explorer() -> AppResult<()> {
    let elevated = win::is_elevated();
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null())
            .map_err(|_| AppError::not_found("El Explorador no se está ejecutando."))?;
        let mut pid = 0u32;
        GetWindowThreadProcessId(tray, Some(&mut pid));
        let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid)?;
        if elevated {
            // Desde un proceso elevado no conviene relanzar el Explorador nosotros
            // (quedaría elevado). Terminarlo hace que Windows lo reinicie solo con
            // el token normal del usuario.
            TerminateProcess(process, 1)?;
        } else {
            PostMessageW(Some(tray), WM_EXIT_EXPLORER, WPARAM(0), LPARAM(0))?;
        }
        WaitForSingleObject(process, 8000);
        let _ = CloseHandle(process);
    }
    if !elevated {
        std::process::Command::new(win::system_root().join("explorer.exe")).spawn()?;
    }
    log::info!(target: "shell", "Se reinició el Explorador");
    Ok(())
}

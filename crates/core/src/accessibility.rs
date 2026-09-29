//! Accesibilidad nativa: la Lupa de Windows (Magnify.exe).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_ESCAPE,
    VK_LWIN,
};

use crate::error::AppResult;
use crate::util::win::{from_wide, system32};
use crate::util::{com, shell};

const MAGNIFIER: &str = "magnify.exe";

/// PID de un proceso por nombre de ejecutable (sin distinguir mayúsculas).
fn find_process(exe: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if from_wide(&entry.szExeFile).eq_ignore_ascii_case(exe) {
                found = Some(entry.th32ProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// ¿La Lupa está abierta?
pub fn magnifier_on() -> bool {
    find_process(MAGNIFIER).is_some()
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

/// Abre o cierra la Lupa.
pub fn set_magnifier(on: bool) -> AppResult<()> {
    if on {
        if magnifier_on() {
            return Ok(());
        }
        let exe = system32("Magnify.exe").to_string_lossy().into_owned();
        com::sta(move || shell::launch(&exe, ""))?;
        log::info!(target: "accesibilidad", "Se abrió la Lupa");
        return Ok(());
    }
    let Some(pid) = find_process(MAGNIFIER) else { return Ok(()) };
    // La Lupa corre con permisos de accesibilidad: si no se puede terminar, se
    // usa su propio atajo de cierre (Win + Esc).
    let closed = unsafe {
        OpenProcess(PROCESS_TERMINATE, false, pid).is_ok_and(|process| {
            let ok = TerminateProcess(process, 0).is_ok();
            let _ = CloseHandle(process);
            ok
        })
    };
    if !closed {
        let inputs = [key(VK_LWIN, false), key(VK_ESCAPE, false), key(VK_ESCAPE, true), key(VK_LWIN, true)];
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    }
    log::info!(target: "accesibilidad", "Se cerró la Lupa");
    Ok(())
}

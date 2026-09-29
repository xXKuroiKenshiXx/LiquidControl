//! Mezclador de volumen por app: sesiones de audio del dispositivo de salida
//! predeterminado (Core Audio, API documentada de Windows). Cada app aparece una
//! sola vez aunque tenga varias sesiones (por ejemplo, varias pestañas sonando).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, S_OK};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{Interface, PWSTR};

use crate::error::{AppError, AppResult};
use crate::util::{com, fileinfo};

const SYSTEM_KEY: &str = "system";

#[derive(Debug, Clone, Serialize)]
pub struct AudioApp {
    /// Clave estable de la app: ruta del ejecutable en minúsculas o "system".
    pub key: String,
    pub name: String,
    pub exe: Option<PathBuf>,
    /// 0–100.
    pub volume: f32,
    pub muted: bool,
    /// Está sonando en este momento.
    pub active: bool,
    pub system: bool,
}

struct Session {
    key: String,
    exe: Option<PathBuf>,
    control: IAudioSessionControl2,
}

fn process_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_FORMAT(0), PWSTR(buffer.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..size as usize])))
    }
}

/// Sesiones vivas del dispositivo de salida, con la clave de su app. Requiere MTA.
fn sessions() -> AppResult<Vec<Session>> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let list = manager.GetSessionEnumerator()?;
        let mut sessions = Vec::new();
        for index in 0..list.GetCount()? {
            let Ok(control) = list.GetSession(index).and_then(|c| c.cast::<IAudioSessionControl2>()) else {
                continue;
            };
            if control.GetState().is_ok_and(|state| state == AudioSessionStateExpired) {
                continue;
            }
            let (key, exe) = if control.IsSystemSoundsSession() == S_OK {
                (SYSTEM_KEY.to_owned(), None)
            } else {
                let pid = control.GetProcessId().unwrap_or(0);
                match process_path(pid) {
                    Some(path) => (path.to_string_lossy().to_lowercase(), Some(path)),
                    None => (format!("pid:{pid}"), None),
                }
            };
            sessions.push(Session { key, exe, control });
        }
        Ok(sessions)
    }
}

/// Nombre para mostrar: el que declara la sesión, la descripción del ejecutable o su nombre.
fn app_name(control: &IAudioSessionControl2, exe: Option<&Path>) -> String {
    let declared = unsafe { control.GetDisplayName() }.ok().and_then(|raw| unsafe {
        let text = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        text
    });
    declared
        .filter(|name| !name.trim().is_empty() && !name.starts_with('@'))
        .or_else(|| exe.and_then(fileinfo::description))
        .or_else(|| exe.and_then(Path::file_stem).map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "App".to_owned())
}

/// Apps con audio abierto: primero las que están sonando.
pub fn apps() -> AppResult<Vec<AudioApp>> {
    com::mta(|| {
        let mut apps: Vec<AudioApp> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for session in sessions()? {
            let active = unsafe { session.control.GetState() }.is_ok_and(|state| state == AudioSessionStateActive);
            if let Some(&index) = by_key.get(&session.key) {
                apps[index].active |= active;
                continue;
            }
            let Ok(volume) = session.control.cast::<ISimpleAudioVolume>() else { continue };
            let (level, muted) = unsafe {
                (volume.GetMasterVolume().unwrap_or(1.0), volume.GetMute().map(|m| m.as_bool()).unwrap_or(false))
            };
            let system = session.key == SYSTEM_KEY;
            let name = if system {
                "Sonidos del sistema".to_owned()
            } else {
                app_name(&session.control, session.exe.as_deref())
            };
            by_key.insert(session.key.clone(), apps.len());
            apps.push(AudioApp {
                key: session.key,
                name,
                exe: session.exe,
                volume: (level * 100.0).round(),
                muted,
                active,
                system,
            });
        }
        apps.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then(a.system.cmp(&b.system))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(apps)
    })
}

/// Aplica `change` a todas las sesiones de una app.
fn for_app(
    key: &str,
    change: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()> + Send + 'static,
) -> AppResult<()> {
    let key = key.to_owned();
    com::mta(move || {
        let mut found = false;
        for session in sessions()?.into_iter().filter(|session| session.key == key) {
            change(&session.control.cast::<ISimpleAudioVolume>()?)?;
            found = true;
        }
        if found { Ok(()) } else { Err(AppError::not_found("Esa app ya no tiene el audio abierto.")) }
    })
}

/// Volumen de una app (0–100), relativo al volumen general.
pub fn set_volume(key: &str, volume: f32) -> AppResult<()> {
    let level = (volume / 100.0).clamp(0.0, 1.0);
    for_app(key, move |control| unsafe { control.SetMasterVolume(level, std::ptr::null()) })
}

pub fn set_mute(key: &str, muted: bool) -> AppResult<()> {
    for_app(key, move |control| unsafe { control.SetMute(muted, std::ptr::null()) })?;
    log::info!(target: "sonido", "{} el audio de {key}", if muted { "Se silenció" } else { "Se reactivó" });
    Ok(())
}

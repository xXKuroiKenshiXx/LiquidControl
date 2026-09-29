//! Notificaciones: interruptores globales y permiso por app.

use serde::Serialize;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY};
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::win::broadcast_setting_change;
use crate::util::{com, reg};

const PUSH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\PushNotifications";
const SETTINGS: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";
const CONTENT: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";

/// Nombres de los emisores del sistema que no están en la carpeta de apps.
const SYSTEM_NAMES: &[(&str, &str)] = &[
    ("Windows.SystemToast.SecurityAndMaintenance", "Seguridad y mantenimiento"),
    ("Windows.SystemToast.AutoPlay", "Reproducción automática"),
    ("Windows.SystemToast.BdeUnlock", "BitLocker"),
    ("Windows.SystemToast.StartupApp", "Aplicaciones de inicio"),
    ("Windows.SystemToast.Suggested", "Sugerencias"),
    ("Windows.SystemToast.Print.Notification", "Impresión"),
    ("Windows.SystemToast.WiFiNetworkManager", "Wi‑Fi"),
    ("Windows.SystemToast.Bluetooth", "Bluetooth"),
    ("Windows.SystemToast.Share", "Compartir"),
    ("Windows.SystemToast.Winlogon", "Inicio de sesión"),
    ("Windows.SystemToast.BackupReminder", "Recordatorios de copia de seguridad"),
    ("Windows.SystemToast.BthQuickPair", "Emparejamiento rápido de Bluetooth"),
    ("Windows.System.AppInitiatedDownload", "Descargas iniciadas por apps"),
    ("Windows.SystemToast.HelloFace", "Windows Hello"),
    ("Windows.SystemToast.Calling", "Llamadas"),
    ("Windows.SystemToast.NfpAppAcquire", "Proximidad (NFC)"),
    ("Windows.SystemToast.WindowsTip", "Consejos de Windows"),
    ("Windows.SystemToast.Devices", "Dispositivos"),
    ("Windows.SystemToast.AudioTroubleshooter", "Solucionador de problemas de audio"),
    ("Windows.SystemToast.LowDisk", "Poco espacio en disco"),
    ("Windows.SystemToast.MobilityExperience", "Continuar desde el teléfono"),
];

#[derive(Debug, Clone, Serialize)]
pub struct NotificationApp {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationsState {
    pub enabled: bool,
    pub lock_screen: bool,
    pub sounds: bool,
    pub tips: bool,
    pub apps: Vec<NotificationApp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationFlag {
    Enabled,
    LockScreen,
    Sounds,
    Tips,
}

fn flag(path: &str, name: &str) -> bool {
    reg::read_dword(path, name).is_none_or(|v| v != 0)
}

/// Nombre legible a partir del identificador cuando el shell no lo conoce:
/// "Editor.MiApp_hash!App" → "Mi App", "Windows.SystemToast.LowDisk" → "Low Disk".
fn fallback_name(aumid: &str) -> String {
    let base = match aumid.split_once('!') {
        Some((family, _)) => family.split('_').next().unwrap_or(family),
        None => aumid,
    };
    let last =
        base.rsplit(['\\', '.']).find(|part| !part.is_empty() && !part.eq_ignore_ascii_case("exe")).unwrap_or(base);
    let mut name = String::with_capacity(last.len() + 4);
    let mut previous_lower = false;
    for ch in last.chars() {
        if ch.is_uppercase() && previous_lower {
            name.push(' ');
        }
        previous_lower = ch.is_lowercase();
        name.push(ch);
    }
    name
}

/// Nombre visible de una app a partir de su AppUserModelID (requiere STA).
fn display_name(aumid: &str) -> String {
    if let Some((_, name)) = SYSTEM_NAMES.iter().find(|(id, _)| *id == aumid) {
        return (*name).to_owned();
    }
    let parsed = unsafe {
        SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(format!(r"shell:AppsFolder\{aumid}")), None)
            .and_then(|item| item.GetDisplayName(SIGDN_NORMALDISPLAY))
            .map(|raw| {
                let name = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.0 as *const _));
                name
            })
    };
    match parsed {
        Ok(name) if !name.is_empty() && !name.contains('!') => name,
        _ => fallback_name(aumid),
    }
}

pub fn get() -> AppResult<NotificationsState> {
    com::sta(|| {
        let mut apps: Vec<NotificationApp> = reg::subkeys(SETTINGS)
            .into_iter()
            .map(|id| NotificationApp {
                name: display_name(&id),
                enabled: flag(&format!(r"{SETTINGS}\{id}"), "Enabled"),
                id,
            })
            .collect();
        apps.sort_by_key(|app| app.name.to_lowercase());
        Ok(NotificationsState {
            enabled: flag(PUSH, "ToastEnabled"),
            lock_screen: flag(SETTINGS, "NOC_GLOBAL_SETTING_ALLOW_TOASTS_ABOVE_LOCK"),
            sounds: flag(SETTINGS, "NOC_GLOBAL_SETTING_ALLOW_NOTIFICATION_SOUND"),
            tips: flag(CONTENT, "SubscribedContent-338389Enabled"),
            apps,
        })
    })
}

pub fn set_flag(flag: NotificationFlag, value: bool) -> AppResult<()> {
    log::info!(target: "notificaciones", "{flag:?}: {}", if value { "sí" } else { "no" });
    let (path, name) = match flag {
        NotificationFlag::Enabled => (PUSH, "ToastEnabled"),
        NotificationFlag::LockScreen => (SETTINGS, "NOC_GLOBAL_SETTING_ALLOW_TOASTS_ABOVE_LOCK"),
        NotificationFlag::Sounds => (SETTINGS, "NOC_GLOBAL_SETTING_ALLOW_NOTIFICATION_SOUND"),
        NotificationFlag::Tips => (CONTENT, "SubscribedContent-338389Enabled"),
    };
    reg::write_dword(path, name, value.into())?;
    broadcast_setting_change(None);
    Ok(())
}

pub fn set_app(id: &str, enabled: bool) -> AppResult<()> {
    log::info!(target: "notificaciones", "{id}: {}", if enabled { "puede avisar" } else { "silenciada" });
    // Solo apps que ya están registradas como emisoras de notificaciones.
    if !reg::subkeys(SETTINGS).iter().any(|key| key == id) {
        return Err(AppError::not_found("Esa app no está registrada para notificaciones."));
    }
    reg::write_dword(&format!(r"{SETTINGS}\{id}"), "Enabled", enabled.into())
}

#[cfg(test)]
mod tests {
    use super::fallback_name;

    #[test]
    fn readable_fallback_names() {
        assert_eq!(fallback_name("Windows.SystemToast.LowDisk"), "Low Disk");
        assert_eq!(fallback_name("Editor.FilesUWP_et10x9a9vyk8t!App"), "Files UWP");
        assert_eq!(fallback_name(r"{6D809377}\Notepad.exe"), "Notepad");
    }
}

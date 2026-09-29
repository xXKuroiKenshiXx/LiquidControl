//! Energía: planes, tiempos de espera, batería e hibernación (API nativa de energía).

use std::ffi::c_void;

use serde::Serialize;
use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, GetSystemPowerStatus, PowerDuplicateScheme, PowerEnumerate, PowerGetActiveScheme,
    PowerReadACValueIndex, PowerReadDCValueIndex, PowerReadFriendlyName, PowerSetActiveScheme, PowerWriteACValueIndex,
    PowerWriteDCValueIndex, SYSTEM_POWER_STATUS,
};
use windows::core::GUID;

use crate::error::{AppError, AppResult};
use crate::util::win::{from_wide, require_admin, system32};
use crate::util::{process, reg};
use windows::Win32::System::Power::{PowerDeleteScheme, PowerWriteFriendlyName};

const MAX_PLAN_NAME: usize = 64;

const VIDEO_SUBGROUP: GUID = GUID::from_u128(0x7516b95f_f776_4464_8c53_06167f40cc99);
const VIDEO_TIMEOUT: GUID = GUID::from_u128(0x3c0bc021_c8a8_4e07_a973_6b14cbcb2b7e);
const SLEEP_SUBGROUP: GUID = GUID::from_u128(0x238c9fa8_0aad_41ed_83f4_97be242c8f20);
const SLEEP_TIMEOUT: GUID = GUID::from_u128(0x29f6c1db_86da_48c5_9fdb_f2b67b1f44da);
/// "Configuración del adaptador inalámbrico" → "Modo de ahorro de energía", el
/// mismo ajuste que el panel clásico de energía de Windows. 0 = máximo
/// rendimiento … 3 = máximo ahorro.
const WIRELESS_SUBGROUP: GUID = GUID::from_u128(0x19cbb8fa_5279_450e_9fac_8a3d5fedd0c1);
const WIRELESS_POWER_SAVING: GUID = GUID::from_u128(0x12bbebe6_58d6_4636_95bb_3217ef867c1a);
/// Plan oculto "Máximo rendimiento" (Ultimate Performance).
pub const ULTIMATE_PLAN: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";
const ULTIMATE: GUID = GUID::from_u128(0xe9a42b02_d5df_448d_aa00_03f14749eb61);
const MAX_TIMEOUT_MINUTES: u32 = 600;

fn check(code: WIN32_ERROR) -> AppResult<()> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(windows::core::Error::from(code.to_hresult()).into()) }
}

fn guid_string(g: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

fn schemes() -> Vec<GUID> {
    let mut list = Vec::new();
    for index in 0.. {
        let mut guid = GUID::zeroed();
        let mut size = size_of::<GUID>() as u32;
        let code = unsafe {
            PowerEnumerate(None, None, None, ACCESS_SCHEME, index, Some(&mut guid as *mut GUID as *mut u8), &mut size)
        };
        if code != ERROR_SUCCESS {
            break;
        }
        list.push(guid);
    }
    list
}

fn friendly_name(guid: &GUID) -> String {
    let mut size = 0u32;
    unsafe {
        let _ = PowerReadFriendlyName(None, Some(guid), None, None, None, &mut size);
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let code =
        unsafe { PowerReadFriendlyName(None, Some(guid), None, None, Some(buffer.as_mut_ptr() as *mut u8), &mut size) };
    if code == ERROR_SUCCESS { from_wide(&buffer) } else { guid_string(guid) }
}

fn active_scheme() -> Option<GUID> {
    let mut ptr: *mut GUID = std::ptr::null_mut();
    let code = unsafe { PowerGetActiveScheme(None, &mut ptr) };
    if code != ERROR_SUCCESS || ptr.is_null() {
        return None;
    }
    let guid = unsafe { *ptr };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(ptr as *mut c_void)));
    }
    Some(guid)
}

fn read_timeout(scheme: &GUID, subgroup: &GUID, setting: &GUID, ac: bool) -> u32 {
    let mut seconds = 0u32;
    // Las variantes AC y DC de la API devuelven tipos distintos: se normaliza a u32.
    let code: u32 = unsafe {
        if ac {
            PowerReadACValueIndex(None, Some(scheme), Some(subgroup), Some(setting), &mut seconds).0
        } else {
            PowerReadDCValueIndex(None, Some(scheme), Some(subgroup), Some(setting), &mut seconds)
        }
    };
    if code == ERROR_SUCCESS.0 { seconds / 60 } else { 0 }
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerPlan {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerStatus {
    pub has_battery: bool,
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub on_ac: bool,
    pub seconds_left: Option<u32>,
    pub plans: Vec<PowerPlan>,
    pub hibernate: bool,
    /// Minutos (0 = nunca).
    pub screen_off_ac: u32,
    pub screen_off_dc: u32,
    pub sleep_ac: u32,
    pub sleep_dc: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    Screen,
    Sleep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Dc,
}

pub fn status() -> AppResult<PowerStatus> {
    let mut battery = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut battery)? };
    // 128 = sin batería, 255 = estado desconocido.
    let has_battery = battery.BatteryFlag != 128 && battery.BatteryFlag != 255;
    let active = active_scheme().unwrap_or_default();
    Ok(PowerStatus {
        has_battery,
        battery_percent: (has_battery && battery.BatteryLifePercent <= 100).then_some(battery.BatteryLifePercent),
        charging: battery.BatteryFlag & 8 != 0,
        on_ac: battery.ACLineStatus == 1,
        seconds_left: (battery.BatteryLifeTime != u32::MAX).then_some(battery.BatteryLifeTime),
        plans: schemes()
            .iter()
            .map(|guid| PowerPlan { guid: guid_string(guid), name: friendly_name(guid), active: *guid == active })
            .collect(),
        hibernate: reg::read_dword(r"HKLM\SYSTEM\CurrentControlSet\Control\Power", "HibernateEnabled") == Some(1),
        screen_off_ac: read_timeout(&active, &VIDEO_SUBGROUP, &VIDEO_TIMEOUT, true),
        screen_off_dc: read_timeout(&active, &VIDEO_SUBGROUP, &VIDEO_TIMEOUT, false),
        sleep_ac: read_timeout(&active, &SLEEP_SUBGROUP, &SLEEP_TIMEOUT, true),
        sleep_dc: read_timeout(&active, &SLEEP_SUBGROUP, &SLEEP_TIMEOUT, false),
    })
}

/// Modo de ahorro de energía del adaptador Wi‑Fi del plan activo (0–3; ver
/// `WIRELESS_POWER_SAVING`). Se aplica igual con o sin corriente.
pub fn wifi_power_saving() -> u32 {
    let active = active_scheme().unwrap_or_default();
    let mut value = 0u32;
    let code = unsafe {
        PowerReadACValueIndex(None, Some(&active), Some(&WIRELESS_SUBGROUP), Some(&WIRELESS_POWER_SAVING), &mut value)
    };
    if code == ERROR_SUCCESS { value.min(3) } else { 0 }
}

pub fn set_wifi_power_saving(level: u32) -> AppResult<()> {
    let level = level.min(3);
    let scheme = active_scheme().ok_or_else(|| AppError::failed("No hay un plan de energía activo."))?;
    let code: u32 = unsafe {
        PowerWriteACValueIndex(None, &scheme, Some(&WIRELESS_SUBGROUP), Some(&WIRELESS_POWER_SAVING), level).0
    };
    check(WIN32_ERROR(code))?;
    let code =
        unsafe { PowerWriteDCValueIndex(None, &scheme, Some(&WIRELESS_SUBGROUP), Some(&WIRELESS_POWER_SAVING), level) };
    check(WIN32_ERROR(code))?;
    // Reactivar el plan aplica el cambio al instante.
    check(unsafe { PowerSetActiveScheme(None, Some(&scheme)) })?;
    const LABELS: [&str; 4] = ["máximo rendimiento", "ahorro bajo", "ahorro medio", "ahorro máximo"];
    log::info!(target: "energía", "Ahorro de energía del Wi‑Fi: {}", LABELS[level as usize]);
    Ok(())
}

fn parse_scheme(guid: &str) -> AppResult<GUID> {
    let parsed = GUID::try_from(guid).map_err(|_| AppError::invalid("Identificador de plan inválido."))?;
    if schemes().contains(&parsed) { Ok(parsed) } else { Err(AppError::not_found("Ese plan de energía no existe.")) }
}

pub fn set_plan(guid: &str) -> AppResult<()> {
    let scheme = parse_scheme(guid)?;
    check(unsafe { PowerSetActiveScheme(None, Some(&scheme)) })?;
    log::info!(target: "energía", "Plan activo: «{}»", friendly_name(&scheme));
    Ok(())
}

/// Duplica un plan y devuelve el GUID de la copia.
fn duplicate(source: &GUID) -> AppResult<GUID> {
    let mut created: *mut GUID = std::ptr::null_mut();
    check(unsafe { PowerDuplicateScheme(None, source, &mut created) })?;
    if created.is_null() {
        return Err(AppError::failed("Windows no devolvió el plan nuevo."));
    }
    let guid = unsafe { *created };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(created as *mut c_void)));
    }
    Ok(guid)
}

/// Agrega el plan oculto "Máximo rendimiento" (duplicándolo) y lo activa.
pub fn add_ultimate() -> AppResult<()> {
    let guid = duplicate(&ULTIMATE)?;
    check(unsafe { PowerSetActiveScheme(None, Some(&guid)) })?;
    log::info!(target: "energía", "Se agregó y activó el plan «Máximo rendimiento»");
    Ok(())
}

fn valid_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_PLAN_NAME || name.chars().any(char::is_control) {
        return Err(AppError::invalid(format!("El nombre debe tener entre 1 y {MAX_PLAN_NAME} caracteres.")));
    }
    Ok(name)
}

fn write_name(scheme: &GUID, name: &str) -> AppResult<()> {
    let bytes: Vec<u8> = name.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    check(unsafe { PowerWriteFriendlyName(None, scheme, None, None, &bytes) })
}

/// Crea un plan nuevo a partir del activo, con el nombre indicado, y lo activa.
pub fn create_plan(name: &str) -> AppResult<String> {
    let name = valid_name(name)?;
    let base = active_scheme().ok_or_else(|| AppError::failed("No hay un plan de energía activo."))?;
    let guid = duplicate(&base)?;
    write_name(&guid, name)?;
    check(unsafe { PowerSetActiveScheme(None, Some(&guid)) })?;
    log::info!(target: "energía", "Se creó el plan «{name}» a partir de «{}»", friendly_name(&base));
    Ok(guid_string(&guid))
}

pub fn rename_plan(guid: &str, name: &str) -> AppResult<()> {
    let scheme = parse_scheme(guid)?;
    let name = valid_name(name)?;
    let previous = friendly_name(&scheme);
    write_name(&scheme, name)?;
    log::info!(target: "energía", "Plan «{previous}» renombrado a «{name}»");
    Ok(())
}

/// Borra un plan. El activo no se puede borrar (primero hay que elegir otro).
pub fn delete_plan(guid: &str) -> AppResult<()> {
    let scheme = parse_scheme(guid)?;
    if active_scheme() == Some(scheme) {
        return Err(AppError::invalid("No se puede borrar el plan activo: elegí otro primero."));
    }
    let name = friendly_name(&scheme);
    check(unsafe { PowerDeleteScheme(None, &scheme) })?;
    log::info!(target: "energía", "Se borró el plan «{name}»");
    Ok(())
}

/// Vuelve a los planes de fábrica de Windows (borra los personalizados).
pub fn restore_default_plans() -> AppResult<()> {
    let output = process::run_hidden(&system32("powercfg.exe"), &["/restoredefaultschemes"])?;
    if output.status.success() {
        log::info!(target: "energía", "Se restauraron los planes de fábrica");
        Ok(())
    } else {
        Err(AppError::failed(format!("powercfg: {}", process::summary(&output))))
    }
}

pub fn set_timeout(kind: TimeoutKind, source: PowerSource, minutes: u32) -> AppResult<()> {
    let scheme = active_scheme().ok_or_else(|| AppError::failed("No hay un plan de energía activo."))?;
    let (subgroup, setting) = match kind {
        TimeoutKind::Screen => (VIDEO_SUBGROUP, VIDEO_TIMEOUT),
        TimeoutKind::Sleep => (SLEEP_SUBGROUP, SLEEP_TIMEOUT),
    };
    let seconds = minutes.min(MAX_TIMEOUT_MINUTES) * 60;
    let code: u32 = unsafe {
        match source {
            PowerSource::Ac => PowerWriteACValueIndex(None, &scheme, Some(&subgroup), Some(&setting), seconds).0,
            PowerSource::Dc => PowerWriteDCValueIndex(None, &scheme, Some(&subgroup), Some(&setting), seconds),
        }
    };
    check(WIN32_ERROR(code))?;
    // Reactivar el plan aplica el cambio al instante.
    check(unsafe { PowerSetActiveScheme(None, Some(&scheme)) })?;
    log::info!(
        target: "energía",
        "{} ({}): {}",
        if kind == TimeoutKind::Screen { "Apagar la pantalla" } else { "Suspender la PC" },
        if source == PowerSource::Ac { "con corriente" } else { "con batería" },
        if minutes == 0 { "nunca".to_owned() } else { format!("{minutes} min") }
    );
    Ok(())
}

pub fn set_hibernate(on: bool) -> AppResult<()> {
    require_admin()?;
    let output = process::run_hidden(&system32("powercfg.exe"), &["/hibernate", if on { "on" } else { "off" }])?;
    if output.status.success() {
        log::info!(target: "energía", "Hibernación {}", if on { "activada" } else { "desactivada" });
        Ok(())
    } else {
        Err(AppError::failed(format!("powercfg: {}", process::summary(&output))))
    }
}

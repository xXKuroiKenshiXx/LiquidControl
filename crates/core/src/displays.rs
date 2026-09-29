//! Pantallas: modos (resolución y frecuencia), escala, posición y brillo por
//! DDC/CI de monitores externos (algo que Windows no ofrece de serie).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DestroyPhysicalMonitors, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, GetMonitorBrightness,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig, SetMonitorBrightness,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CDS_TEST, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY,
    DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayMonitors,
    EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::{BOOL, HSTRING};

use crate::error::{AppError, AppResult};
use crate::util::process;
use crate::util::win::from_wide;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DisplayConfigSetDeviceInfo,
};
use windows::Win32::Foundation::LUID;
use windows::core::HRESULT;

const MONITORINFOF_PRIMARY: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplayInfo {
    /// Dispositivo GDI ("\\.\DISPLAY1").
    pub id: String,
    pub name: String,
    pub primary: bool,
    pub x: i32,
    pub y: i32,
    pub mode: DisplayMode,
    /// Escala en % (100, 125, 150…).
    pub scale: u32,
    pub modes: Vec<DisplayMode>,
    /// Brillo 0–100 si el monitor admite DDC/CI.
    pub brightness: Option<u32>,
    /// HDR activado (`None` si la pantalla no lo admite).
    pub hdr: Option<bool>,
}

struct MonitorRef {
    handle: HMONITOR,
    device: String,
    primary: bool,
}

fn monitors() -> Vec<MonitorRef> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorRef>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
            list.push(MonitorRef {
                handle: monitor,
                device: from_wide(&info.szDevice),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        true.into()
    }
    let mut list: Vec<MonitorRef> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut Vec<MonitorRef> as isize));
    }
    list
}

/// Nombre comercial del monitor ("LG ULTRAGEAR") por dispositivo GDI ("\\.\DISPLAY1").
fn friendly_names() -> HashMap<String, String> {
    let mut names = HashMap::new();
    for path in active_paths() {
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        target.header.adapterId = path.targetInfo.adapterId;
        target.header.id = path.targetInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } != 0 {
            continue;
        }
        let name = from_wide(&target.monitorFriendlyDeviceName);
        if let (false, Some(device)) = (name.is_empty(), gdi_name(&path)) {
            names.insert(device, name);
        }
    }
    names
}

/// Caminos de pantalla activos (fuente → monitor) según la API DisplayConfig.
fn active_paths() -> Vec<DISPLAYCONFIG_PATH_INFO> {
    let (mut path_count, mut mode_count) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return Vec::new();
        }
        paths.truncate(path_count as usize);
        paths
    }
}

/// Dispositivo GDI ("\\.\DISPLAY1") de un camino.
fn gdi_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    source.header.adapterId = path.sourceInfo.adapterId;
    source.header.id = path.sourceInfo.id;
    (unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0).then(|| from_wide(&source.viewGdiDeviceName))
}

/// Monitor (adaptador e id) de cada dispositivo GDI, para las funciones de color.
fn targets() -> HashMap<String, (LUID, u32)> {
    active_paths()
        .iter()
        .filter_map(|path| Some((gdi_name(path)?, (path.targetInfo.adapterId, path.targetInfo.id))))
        .collect()
}

/// `Some(activado)` si el monitor admite HDR (color avanzado).
fn hdr_state((adapter, id): (LUID, u32)) -> Option<bool> {
    let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
    info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
    info.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
    info.header.adapterId = adapter;
    info.header.id = id;
    if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != 0 {
        return None;
    }
    // Bit 0: lo admite; bit 1: está activado.
    let bits = unsafe { info.Anonymous.value };
    (bits & 1 != 0).then_some(bits & 2 != 0)
}

/// Activa o desactiva el HDR de una pantalla.
pub fn set_hdr(id: &str, on: bool) -> AppResult<()> {
    let target = *targets().get(id).ok_or_else(|| AppError::not_found("Esa pantalla ya no está conectada."))?;
    if hdr_state(target).is_none() {
        return Err(AppError::invalid("Esta pantalla no admite HDR."));
    }
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
    state.header.r#type = DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE;
    state.header.size = size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32;
    state.header.adapterId = target.0;
    state.header.id = target.1;
    state.Anonymous.value = u32::from(on);
    let code = unsafe { DisplayConfigSetDeviceInfo(&state.header) };
    if code != 0 {
        return Err(windows::core::Error::from(HRESULT::from_win32(code as u32)).into());
    }
    log::info!(target: "pantallas", "HDR {} en {id}", if on { "activado" } else { "desactivado" });
    Ok(())
}

fn devmode() -> DEVMODEW {
    DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() }
}

fn current_mode(device: &HSTRING) -> Option<(DisplayMode, i32, i32)> {
    let mut mode = devmode();
    unsafe {
        if !EnumDisplaySettingsW(device, ENUM_CURRENT_SETTINGS, &mut mode).as_bool() {
            return None;
        }
        let position = mode.Anonymous1.Anonymous2.dmPosition;
        Some((
            DisplayMode { width: mode.dmPelsWidth, height: mode.dmPelsHeight, hz: mode.dmDisplayFrequency },
            position.x,
            position.y,
        ))
    }
}

fn modes_for(device: &HSTRING) -> Vec<DisplayMode> {
    let mut set = BTreeSet::new();
    for index in 0.. {
        let mut mode = devmode();
        if !unsafe { EnumDisplaySettingsW(device, ENUM_DISPLAY_SETTINGS_MODE(index), &mut mode) }.as_bool() {
            break;
        }
        if mode.dmBitsPerPel >= 24 && mode.dmPelsWidth >= 800 {
            set.insert(DisplayMode { width: mode.dmPelsWidth, height: mode.dmPelsHeight, hz: mode.dmDisplayFrequency });
        }
    }
    set.into_iter().rev().collect()
}

/// Ejecuta `work` con el primer monitor físico (DDC/CI) de un HMONITOR.
fn with_physical<T>(monitor: HMONITOR, work: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
    unsafe {
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).ok()?;
        if count == 0 {
            return None;
        }
        let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical).ok()?;
        let result = work(physical[0].hPhysicalMonitor);
        let _ = DestroyPhysicalMonitors(&physical);
        result
    }
}

fn brightness(monitor: HMONITOR) -> Option<u32> {
    with_physical(monitor, |handle| {
        let (mut min, mut current, mut max) = (0u32, 0u32, 0u32);
        let ok = unsafe { GetMonitorBrightness(handle, &mut min, &mut current, &mut max) } != 0;
        (ok && max > min).then(|| (current.saturating_sub(min)) * 100 / (max - min))
    })
}

/// Pantallas conectadas, con la principal primero.
pub fn list() -> Vec<DisplayInfo> {
    let names = friendly_names();
    let targets = targets();
    let mut list: Vec<DisplayInfo> = monitors()
        .into_iter()
        .filter_map(|monitor| {
            let device = HSTRING::from(monitor.device.as_str());
            let (mode, x, y) = current_mode(&device)?;
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            unsafe {
                let _ = GetDpiForMonitor(monitor.handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            }
            Some(DisplayInfo {
                name: names
                    .get(&monitor.device)
                    .cloned()
                    .unwrap_or_else(|| monitor.device.trim_start_matches(r"\\.\").to_owned()),
                id: monitor.device.clone(),
                primary: monitor.primary,
                x,
                y,
                mode,
                scale: dpi_x * 100 / 96,
                modes: modes_for(&device),
                brightness: brightness(monitor.handle),
                hdr: targets.get(&monitor.device).copied().and_then(hdr_state),
            })
        })
        .collect();
    list.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.x.cmp(&b.x)));
    list
}

/// Cambia resolución y frecuencia. Solo acepta modos que el monitor anuncia.
pub fn set_mode(id: &str, mode: DisplayMode) -> AppResult<()> {
    log::info!(target: "pantallas", "Modo de {id}: {} × {} a {} Hz", mode.width, mode.height, mode.hz);
    if !monitors().iter().any(|m| m.device == id) {
        return Err(AppError::not_found("Esa pantalla ya no está conectada."));
    }
    let device = HSTRING::from(id);
    if !modes_for(&device).contains(&mode) {
        return Err(AppError::invalid("El monitor no admite ese modo."));
    }
    let mut devmode = devmode();
    unsafe {
        if !EnumDisplaySettingsW(&device, ENUM_CURRENT_SETTINGS, &mut devmode).as_bool() {
            return Err(AppError::failed("No se pudo leer el modo actual."));
        }
    }
    devmode.dmPelsWidth = mode.width;
    devmode.dmPelsHeight = mode.height;
    devmode.dmDisplayFrequency = mode.hz;
    devmode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
    unsafe {
        // Primero se prueba: si el driver no lo acepta, no se toca nada.
        if ChangeDisplaySettingsExW(&device, Some(&devmode), None, CDS_TEST, None) != DISP_CHANGE_SUCCESSFUL {
            return Err(AppError::invalid("El monitor no admite ese modo."));
        }
        if ChangeDisplaySettingsExW(&device, Some(&devmode), None, CDS_UPDATEREGISTRY, None) != DISP_CHANGE_SUCCESSFUL {
            return Err(AppError::failed("Windows no pudo cambiar el modo de pantalla."));
        }
    }
    Ok(())
}

// ─────────────────────────── Luz nocturna ───────────────────────────
//
// El estado real no vive en un valor simple del registro: son dos blobs
// binarios opacos que Windows arma y lee él solo (uno para prendido/apagado
// y su reloj, otro para la temperatura de color). El formato se reversó
// públicamente (github.com/nathanbabcock/nightlight-cli, a su vez basado en
// github.com/Maclay74/tiny-screen); esto reimplementa esa misma lógica byte
// por byte en PowerShell, sin "mejorarla", para no romper algo tan frágil.

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NightLight {
    pub supported: bool,
    pub enabled: bool,
    /// Intensidad 0–100 (0 = neutro/6500K, 100 = lo más cálido/1200K).
    pub strength: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawNightLight {
    supported: bool,
    enabled: bool,
    strength: u32,
}

/// Funciones de PowerShell compartidas por los tres scripts de esta sección
/// (leer, prender/apagar, cambiar intensidad): puerto directo de
/// `Switch-NightLight.psm1` (ver comentario de arriba), variable por
/// variable, para no arriesgar un byte mal puesto reescribiéndolo distinto
/// en cada script.
const NIGHT_LIGHT_FUNCTIONS: &str = r#"
$stateKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default`$windows.data.bluelightreduction.bluelightreductionstate\windows.data.bluelightreduction.bluelightreductionstate"
$settingsKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default`$windows.data.bluelightreduction.settings\windows.data.bluelightreduction.settings"
$MIN_KELVIN = 1200
$MAX_KELVIN = 6500

function Test-NightLightSupported {
    return (Test-Path -Path $stateKeyPath) -and (Test-Path -Path $settingsKeyPath)
}
function Get-NightLightData {
    (Get-ItemProperty -Path $stateKeyPath -Name "Data" -ErrorAction Stop).Data
}
function Test-NightLightEnabled {
    $data = Get-NightLightData
    return $null -ne $data -and $data[18] -eq 0x15
}
function Switch-NightLight {
    $enabled = Test-NightLightEnabled
    $data = Get-NightLightData
    if ($enabled) {
        $newData = New-Object byte[] 41
        [Array]::Copy($data, 0, $newData, 0, [Math]::Min(22, $data.Length))
        if ($data.Length -gt 25) {
            $copyLength = [Math]::Min($data.Length - 25, 43 - 25)
            [Array]::Copy($data, 25, $newData, 23, $copyLength)
        }
        $newData[18] = 0x13
    } else {
        $newData = New-Object byte[] 43
        [Array]::Copy($data, 0, $newData, 0, [Math]::Min(22, $data.Length))
        if ($data.Length -gt 23) {
            $copyLength = [Math]::Min($data.Length - 23, 41 - 23)
            [Array]::Copy($data, 23, $newData, 25, $copyLength)
        }
        $newData[18] = 0x15
        $newData[23] = 0x10
        $newData[24] = 0x00
    }
    for ($i = 10; $i -lt 15; $i++) {
        if ($newData[$i] -ne 0xff) { $newData[$i]++; break }
    }
    Set-ItemProperty -Path $stateKeyPath -Name "Data" -Value $newData -Type Binary
}
function ConvertFrom-Kelvin($kelvin) {
    return 100 - (($kelvin - $MIN_KELVIN) / ($MAX_KELVIN - $MIN_KELVIN)) * 100
}
function ConvertTo-Kelvin($percentage) {
    return $MAX_KELVIN - ($percentage / 100) * ($MAX_KELVIN - $MIN_KELVIN)
}
function Get-NightLightStrength {
    $data = (Get-ItemProperty -Path $settingsKeyPath -Name "Data" -ErrorAction Stop).Data
    if ($null -eq $data -or $data.Length -le 0x24) { return 0 }
    $tempLo = $data[0x23]
    $tempHi = $data[0x24]
    $kelvin = ($tempHi * 64) + (($tempLo - 128) / 2)
    return [Math]::Max(0, [Math]::Min(100, [Math]::Round((ConvertFrom-Kelvin $kelvin))))
}
function Set-NightLightStrength([int]$Percentage) {
    $data = (Get-ItemProperty -Path $settingsKeyPath -Name "Data" -ErrorAction Stop).Data
    $kelvin = ConvertTo-Kelvin $Percentage
    $tempHi = [Math]::Floor($kelvin / 64)
    $tempLo = (($kelvin - ($tempHi * 64)) * 2) + 128
    $newData = $data.Clone()
    $newData[0x23] = $tempLo
    $newData[0x24] = $tempHi
    for ($i = 10; $i -lt 15; $i++) {
        if ($newData[$i] -ne 0xff) { $newData[$i]++; break }
    }
    Set-ItemProperty -Path $settingsKeyPath -Name "Data" -Value $newData -Type Binary
}
"#;

fn night_light_script(body: &str) -> String {
    format!("{NIGHT_LIGHT_FUNCTIONS}\n{body}")
}

/// Estado actual: prendida/apagada e intensidad (0–100). `supported: false`
/// si esta versión de Windows no guarda el estado en este formato (no se
/// toca nada en ese caso).
pub fn night_light() -> AppResult<NightLight> {
    let script = night_light_script(
        r#"
        $supported = Test-NightLightSupported
        $enabled = if ($supported) { Test-NightLightEnabled } else { $false }
        $strength = if ($supported) { Get-NightLightStrength } else { 0 }
        [pscustomobject]@{ Supported = $supported; Enabled = $enabled; Strength = $strength } |
            ConvertTo-Json -Compress
        "#,
    );
    let output = process::powershell("leer luz nocturna", &script)?;
    let raw: RawNightLight = serde_json::from_str(output.trim())
        .map_err(|err| AppError::failed(format!("No se pudo leer la luz nocturna: {err}")))?;
    Ok(NightLight { supported: raw.supported, enabled: raw.enabled, strength: raw.strength })
}

/// Prende o apaga (no hace nada si ya está en ese estado: el "switch" real
/// alterna, así que solo se llama cuando hace falta).
pub fn set_night_light_enabled(on: bool) -> AppResult<()> {
    log::info!(target: "pantallas", "Luz nocturna: {}", if on { "on" } else { "off" });
    let target = if on { "$true" } else { "$false" };
    let script = night_light_script(&format!(
        r#"
        if (-not (Test-NightLightSupported)) {{ exit }}
        if ((Test-NightLightEnabled) -ne {target}) {{ Switch-NightLight }}
        "#
    ));
    process::powershell("luz nocturna on/off", &script)?;
    Ok(())
}

/// Intensidad 0–100.
pub fn set_night_light_strength(percent: u32) -> AppResult<()> {
    let percent = percent.min(100);
    log::info!(target: "pantallas", "Intensidad de luz nocturna: {percent}");
    let script = night_light_script(&format!(
        r#"
        if (-not (Test-NightLightSupported)) {{ exit }}
        Set-NightLightStrength {percent}
        "#
    ));
    process::powershell("intensidad de luz nocturna", &script)?;
    Ok(())
}

/// Brillo por DDC/CI (0–100).
pub fn set_brightness(id: &str, value: u32) -> AppResult<()> {
    log::info!(target: "pantallas", "Brillo de {id}: {} %", value.min(100));
    let monitor = monitors()
        .into_iter()
        .find(|m| m.device == id)
        .ok_or_else(|| AppError::not_found("Esa pantalla ya no está conectada."))?;
    with_physical(monitor.handle, |handle| unsafe {
        let (mut min, mut current, mut max) = (0u32, 0u32, 0u32);
        if GetMonitorBrightness(handle, &mut min, &mut current, &mut max) == 0 || max <= min {
            return None;
        }
        let target = min + (max - min) * value.min(100) / 100;
        (SetMonitorBrightness(handle, target) != 0).then_some(())
    })
    .ok_or_else(|| AppError::failed("El monitor no respondió por DDC/CI."))
}

//! Mouse y teclado: velocidad, precisión, botones, rueda, cursor, repetición.

use std::ffi::c_void;

use serde::Serialize;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCaretBlinkTime, GetSystemMetrics, SM_SWAPBUTTON, SPI_GETCARETWIDTH, SPI_GETKEYBOARDDELAY, SPI_GETKEYBOARDSPEED,
    SPI_GETMOUSE, SPI_GETMOUSESPEED, SPI_GETWHEELSCROLLLINES, SPI_SETCARETWIDTH, SPI_SETCURSORS,
    SPI_SETDOUBLECLICKTIME, SPI_SETKEYBOARDDELAY, SPI_SETKEYBOARDSPEED, SPI_SETMOUSE, SPI_SETMOUSEBUTTONSWAP,
    SPI_SETMOUSESPEED, SPI_SETWHEELSCROLLLINES, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SYSTEM_PARAMETERS_INFO_ACTION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCaretBlinkTime, SystemParametersInfoW,
};

use crate::error::AppResult;
use crate::util::reg;
use crate::util::win::{broadcast_setting_change, require_admin};

const DESKTOP: &str = r"HKCU\Control Panel\Desktop";
const CURSORS: &str = r"HKCU\Control Panel\Cursors";
const ACCESSIBILITY: &str = r"HKCU\Software\Microsoft\Accessibility";
const HID_ENUM: &str = r"HKLM\SYSTEM\CurrentControlSet\Enum\HID";
/// SPI no documentado que aplica el tamaño del cursor al instante (Windows 10+).
const SPI_SETCURSORSIZE: SYSTEM_PARAMETERS_INFO_ACTION = SYSTEM_PARAMETERS_INFO_ACTION(0x2029);

fn persist() -> SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS {
    SPIF_UPDATEINIFILE | SPIF_SENDCHANGE
}

fn spi_get_u32(action: SYSTEM_PARAMETERS_INFO_ACTION) -> Option<u32> {
    let mut value = 0u32;
    unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some(&mut value as *mut u32 as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()
    .map(|_| value)
}

/// Para SPIs que reciben el valor en `pvParam` (no un puntero).
fn spi_set_value(action: SYSTEM_PARAMETERS_INFO_ACTION, value: u32) -> AppResult<()> {
    unsafe { SystemParametersInfoW(action, 0, Some(value as usize as *mut c_void), persist())? };
    Ok(())
}

/// Para SPIs que reciben el valor en `uiParam`.
fn spi_set_ui(action: SYSTEM_PARAMETERS_INFO_ACTION, value: u32) -> AppResult<()> {
    unsafe { SystemParametersInfoW(action, value, None, persist())? };
    Ok(())
}

// ─────────────────────────── Mouse ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MouseState {
    pub speed: u32,
    pub enhance_precision: bool,
    pub swap_buttons: bool,
    pub double_click_ms: u32,
    pub wheel_lines: u32,
    pub scroll_inactive: bool,
    pub cursor_size: u32,
    /// `None` = no se detectó un mouse compatible.
    pub natural_scroll: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub enum MouseSetting {
    Speed(u32),
    EnhancePrecision(bool),
    SwapButtons(bool),
    DoubleClickMs(u32),
    WheelLines(u32),
    ScrollInactive(bool),
    CursorSize(u32),
    NaturalScroll(bool),
}

/// Claves "Device Parameters" de los dispositivos HID con rueda (mouses).
fn wheel_devices() -> Vec<String> {
    let mut keys = Vec::new();
    for device in reg::subkeys(HID_ENUM) {
        let device_path = format!(r"{HID_ENUM}\{device}");
        for instance in reg::subkeys(&device_path) {
            let params = format!(r"{device_path}\{instance}\Device Parameters");
            if reg::read_dword(&params, "FlipFlopWheel").is_some() {
                keys.push(params);
            }
        }
    }
    keys
}

fn natural_scroll_state() -> Option<bool> {
    let devices = wheel_devices();
    (!devices.is_empty()).then(|| devices.iter().all(|key| reg::read_dword(key, "FlipFlopWheel") == Some(1)))
}

pub fn mouse() -> MouseState {
    let mut accel = [0i32; 3];
    let precision = unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSE,
            0,
            Some(accel.as_mut_ptr() as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && accel[2] != 0;
    MouseState {
        speed: spi_get_u32(SPI_GETMOUSESPEED).unwrap_or(10),
        enhance_precision: precision,
        swap_buttons: unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0,
        double_click_ms: unsafe { GetDoubleClickTime() },
        wheel_lines: spi_get_u32(SPI_GETWHEELSCROLLLINES).unwrap_or(3).min(20),
        scroll_inactive: reg::read_dword(DESKTOP, "MouseWheelRouting").is_none_or(|v| v == 2),
        cursor_size: reg::read_dword(CURSORS, "CursorBaseSize").unwrap_or(32),
        natural_scroll: natural_scroll_state(),
    }
}

pub fn set_mouse(setting: MouseSetting) -> AppResult<()> {
    log::info!(target: "mouse", "{setting:?}");
    match setting {
        MouseSetting::Speed(speed) => spi_set_value(SPI_SETMOUSESPEED, speed.clamp(1, 20)),
        MouseSetting::EnhancePrecision(on) => {
            // Umbrales estándar de Windows con la aceleración activada.
            let mut accel: [i32; 3] = if on { [6, 10, 1] } else { [0, 0, 0] };
            unsafe { SystemParametersInfoW(SPI_SETMOUSE, 0, Some(accel.as_mut_ptr() as *mut c_void), persist())? };
            Ok(())
        }
        MouseSetting::SwapButtons(on) => spi_set_ui(SPI_SETMOUSEBUTTONSWAP, on.into()),
        MouseSetting::DoubleClickMs(ms) => spi_set_ui(SPI_SETDOUBLECLICKTIME, ms.clamp(200, 900)),
        MouseSetting::WheelLines(lines) => spi_set_ui(SPI_SETWHEELSCROLLLINES, lines.clamp(1, 100)),
        MouseSetting::ScrollInactive(on) => {
            reg::write_dword(DESKTOP, "MouseWheelRouting", if on { 2 } else { 0 })?;
            broadcast_setting_change(None);
            Ok(())
        }
        MouseSetting::CursorSize(size) => {
            let size = (size.clamp(32, 256) / 16) * 16;
            reg::write_dword(CURSORS, "CursorBaseSize", size)?;
            reg::write_dword(ACCESSIBILITY, "CursorSize", (size - 32) / 16 + 1)?;
            unsafe {
                let _ = SystemParametersInfoW(SPI_SETCURSORSIZE, 0, Some(size as usize as *mut c_void), persist());
                SystemParametersInfoW(SPI_SETCURSORS, 0, None, persist())?;
            }
            Ok(())
        }
        MouseSetting::NaturalScroll(on) => {
            require_admin()?;
            for key in wheel_devices() {
                reg::write_dword(&key, "FlipFlopWheel", on.into())?;
                reg::write_dword(&key, "FlipFlopHScroll", on.into())?;
            }
            Ok(())
        }
    }
}

// ─────────────────────────── Teclado ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct KeyboardState {
    pub repeat_delay: u32,
    pub repeat_rate: u32,
    pub caret_blink_ms: u32,
    pub caret_width: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum KeyboardSetting {
    RepeatDelay(u32),
    RepeatRate(u32),
    CaretBlinkMs(u32),
    CaretWidth(u32),
}

pub fn keyboard() -> KeyboardState {
    let blink = unsafe { GetCaretBlinkTime() };
    KeyboardState {
        repeat_delay: spi_get_u32(SPI_GETKEYBOARDDELAY).unwrap_or(1),
        repeat_rate: spi_get_u32(SPI_GETKEYBOARDSPEED).unwrap_or(31),
        // INFINITE (sin parpadeo) se muestra como el valor más lento.
        caret_blink_ms: if blink == u32::MAX { 1200 } else { blink.clamp(200, 1200) },
        caret_width: spi_get_u32(SPI_GETCARETWIDTH).unwrap_or(1),
    }
}

pub fn set_keyboard(setting: KeyboardSetting) -> AppResult<()> {
    log::info!(target: "teclado", "{setting:?}");
    match setting {
        KeyboardSetting::RepeatDelay(delay) => spi_set_ui(SPI_SETKEYBOARDDELAY, delay.min(3)),
        KeyboardSetting::RepeatRate(rate) => spi_set_ui(SPI_SETKEYBOARDSPEED, rate.min(31)),
        KeyboardSetting::CaretBlinkMs(ms) => {
            let ms = ms.clamp(200, 1200);
            unsafe { SetCaretBlinkTime(ms)? };
            reg::write_string(DESKTOP, "CursorBlinkRate", &ms.to_string())?;
            Ok(())
        }
        KeyboardSetting::CaretWidth(width) => spi_set_value(SPI_SETCARETWIDTH, width.clamp(1, 20)),
    }
}

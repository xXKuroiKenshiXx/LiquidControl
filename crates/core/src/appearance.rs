//! Apariencia de Windows: modo claro/oscuro, acento, transparencia, animaciones.

use std::ffi::c_void;

use serde::Serialize;
use windows::Win32::UI::WindowsAndMessaging::{
    ANIMATIONINFO, SPI_GETCLIENTAREAANIMATION, SPI_SETANIMATION, SPI_SETCLIENTAREAANIMATION, SPIF_SENDCHANGE,
    SPIF_UPDATEINIFILE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

use crate::error::AppResult;
use crate::util::reg;
use crate::util::win::broadcast_setting_change;

const PERSONALIZE: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const DWM: &str = r"HKCU\Software\Microsoft\Windows\DWM";
const ACCENT: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";
const DESKTOP: &str = r"HKCU\Control Panel\Desktop";
const ACCESSIBILITY: &str = r"HKCU\Control Panel\Accessibility";
const THEME_CHANGED: &str = "ImmersiveColorSet";

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Serialize)]
pub struct AppearanceState {
    pub apps_light: bool,
    pub system_light: bool,
    pub transparency: bool,
    pub accent_on_taskbar: bool,
    pub accent_on_title_bars: bool,
    pub animations: bool,
    pub always_show_scrollbars: bool,
    pub accent: Rgb,
    /// Windows elige el acento según el fondo de pantalla.
    pub auto_accent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceFlag {
    Transparency,
    AccentOnTaskbar,
    AccentOnTitleBars,
    Animations,
    AlwaysShowScrollbars,
}

impl AppearanceFlag {
    fn label(self) -> &'static str {
        match self {
            Self::Transparency => "Efectos de transparencia",
            Self::AccentOnTaskbar => "Acento en Inicio y la barra de tareas",
            Self::AccentOnTitleBars => "Acento en barras de título",
            Self::Animations => "Efectos de animación",
            Self::AlwaysShowScrollbars => "Barras de desplazamiento siempre visibles",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

fn flag(path: &str, name: &str, default: bool) -> bool {
    reg::read_dword(path, name).map_or(default, |v| v != 0)
}

fn client_area_animation() -> bool {
    let mut enabled: i32 = 1;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut i32 as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && enabled != 0
}

/// Color de acento de Windows como (R, G, B).
pub fn accent_rgb() -> Rgb {
    // AccentColor se guarda como 0xAABBGGRR; ColorizationColor como 0xAARRGGBB.
    if let Some(abgr) = reg::read_dword(DWM, "AccentColor") {
        return (abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8);
    }
    if let Some(argb) = reg::read_dword(DWM, "ColorizationColor") {
        return ((argb >> 16) as u8, (argb >> 8) as u8, argb as u8);
    }
    (0x00, 0x78, 0xD4)
}

pub fn get() -> AppearanceState {
    AppearanceState {
        apps_light: flag(PERSONALIZE, "AppsUseLightTheme", true),
        system_light: flag(PERSONALIZE, "SystemUsesLightTheme", false),
        transparency: flag(PERSONALIZE, "EnableTransparency", true),
        accent_on_taskbar: flag(PERSONALIZE, "ColorPrevalence", false),
        accent_on_title_bars: flag(DWM, "ColorPrevalence", false),
        animations: client_area_animation(),
        always_show_scrollbars: reg::read_dword(ACCESSIBILITY, "DynamicScrollbars") == Some(0),
        accent: accent_rgb(),
        auto_accent: reg::read_dword(DESKTOP, "AutoColorization") == Some(1),
    }
}

/// ¿Windows usa el modo oscuro para las apps? (la app sigue este valor por defecto).
pub fn apps_use_dark() -> bool {
    !flag(PERSONALIZE, "AppsUseLightTheme", true)
}

pub fn set_theme(apps: Mode, system: Mode) -> AppResult<()> {
    reg::write_dword(PERSONALIZE, "AppsUseLightTheme", (apps == Mode::Light).into())?;
    reg::write_dword(PERSONALIZE, "SystemUsesLightTheme", (system == Mode::Light).into())?;
    broadcast_setting_change(Some(THEME_CHANGED));
    log::info!(target: "apariencia", "Apps en modo {:?}, Windows en modo {:?}", apps, system);
    Ok(())
}

fn set_animations(enabled: bool) -> AppResult<()> {
    let flags = SPIF_UPDATEINIFILE | SPIF_SENDCHANGE;
    unsafe {
        SystemParametersInfoW(SPI_SETCLIENTAREAANIMATION, 0, Some(enabled as usize as *mut c_void), flags)?;
        let mut info = ANIMATIONINFO { cbSize: size_of::<ANIMATIONINFO>() as u32, iMinAnimate: enabled as i32 };
        SystemParametersInfoW(
            SPI_SETANIMATION,
            info.cbSize,
            Some(&mut info as *mut ANIMATIONINFO as *mut c_void),
            flags,
        )?;
    }
    Ok(())
}

pub fn set_flag(flag: AppearanceFlag, value: bool) -> AppResult<()> {
    match flag {
        AppearanceFlag::Transparency => reg::write_dword(PERSONALIZE, "EnableTransparency", value.into())?,
        AppearanceFlag::AccentOnTaskbar => reg::write_dword(PERSONALIZE, "ColorPrevalence", value.into())?,
        AppearanceFlag::AccentOnTitleBars => reg::write_dword(DWM, "ColorPrevalence", value.into())?,
        AppearanceFlag::Animations => set_animations(value)?,
        AppearanceFlag::AlwaysShowScrollbars => reg::write_dword(ACCESSIBILITY, "DynamicScrollbars", (!value).into())?,
    }
    broadcast_setting_change(if flag == AppearanceFlag::AlwaysShowScrollbars { None } else { Some(THEME_CHANGED) });
    log::info!(target: "apariencia", "{}: {}", flag.label(), if value { "sí" } else { "no" });
    Ok(())
}

fn mix((r, g, b): Rgb, target: u8, amount: f32) -> Rgb {
    let channel = |c: u8| (f32::from(c) + (f32::from(target) - f32::from(c)) * amount).round() as u8;
    (channel(r), channel(g), channel(b))
}

/// Paleta de 8 tonos como la que arma Windows: tres más claros, el acento, tres más oscuros y un gris.
fn palette(color: Rgb) -> [Rgb; 8] {
    [
        mix(color, 255, 0.6),
        mix(color, 255, 0.4),
        mix(color, 255, 0.2),
        color,
        mix(color, 0, 0.2),
        mix(color, 0, 0.4),
        mix(color, 0, 0.6),
        (0x88, 0x88, 0x88),
    ]
}

fn abgr((r, g, b): Rgb) -> u32 {
    0xFF00_0000 | (u32::from(b) << 16) | (u32::from(g) << 8) | u32::from(r)
}

fn argb(alpha: u8, (r, g, b): Rgb) -> u32 {
    (u32::from(alpha) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Cambia el color de acento de Windows (Inicio, barra de tareas, bordes y apps).
pub fn set_accent(color: Rgb) -> AppResult<()> {
    let shades = palette(color);
    let bytes: Vec<u8> = shades.iter().flat_map(|&(r, g, b)| [r, g, b, 0]).collect();
    reg::write_dword(DESKTOP, "AutoColorization", 0)?;
    reg::write_binary(ACCENT, "AccentPalette", &bytes)?;
    reg::write_dword(ACCENT, "AccentColorMenu", abgr(shades[3]))?;
    reg::write_dword(ACCENT, "StartColorMenu", abgr(shades[5]))?;
    reg::write_dword(DWM, "AccentColor", abgr(shades[3]))?;
    reg::write_dword(DWM, "ColorizationColor", argb(0xC4, shades[3]))?;
    reg::write_dword(DWM, "ColorizationAfterglow", argb(0xC4, shades[3]))?;
    broadcast_setting_change(Some(THEME_CHANGED));
    let (r, g, b) = color;
    log::info!(target: "apariencia", "Color de acento: #{r:02X}{g:02X}{b:02X}");
    Ok(())
}

/// Que Windows elija el acento según el fondo de pantalla.
pub fn set_auto_accent(on: bool) -> AppResult<()> {
    reg::write_dword(DESKTOP, "AutoColorization", on.into())?;
    broadcast_setting_change(Some(THEME_CHANGED));
    log::info!(target: "apariencia", "Acento automático según el fondo: {}", if on { "sí" } else { "no" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{abgr, argb, palette};

    #[test]
    fn accent_palette_and_formats() {
        let shades = palette((0x00, 0x78, 0xD4));
        assert_eq!(shades[3], (0x00, 0x78, 0xD4));
        assert!(shades[0].2 > shades[3].2 && shades[6].2 < shades[3].2);
        assert_eq!(abgr((0x11, 0x22, 0x33)), 0xFF33_2211);
        assert_eq!(argb(0xC4, (0x11, 0x22, 0x33)), 0xC411_2233);
    }
}

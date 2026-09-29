//! Forma de la ventana: las esquinas nativas de Windows 11 o esquinas más
//! grandes al estilo macOS.
//!
//! Windows solo redondea las ventanas con un radio fijo (8 px). Para un radio
//! mayor se recorta la ventana con una región redondeada; el sistema no suaviza
//! el borde de una región ni le dibuja sombra, por eso es una opción y no el
//! valor predeterminado.

use core::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, GetMonitorInfoW, HRGN, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, SetWindowRgn,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

// NOTA: se probó `DwmExtendFrameIntoClientArea` con margen 0 para sacar una
// línea blanca que a veces queda pegada arriba de la ventana, pero resultó
// ser al revés: esa función es una causa CONOCIDA de líneas/bordes blancos
// en ventanas sin marco con estilo `WS_THICKFRAME` (el que usa una ventana
// redimensionable como esta) — hasta el equipo de Windows Terminal la dejó
// de usar por el mismo motivo y pasó a dibujar el borde ellos mismos
// (github.com/microsoft/terminal/pull/4577). Se sacó de acá; si la línea
// blanca sigue apareciendo hace falta ese enfoque (dibujar el borde propio),
// no esta función.

fn set_corner_preference(hwnd: HWND, preference: DWM_WINDOW_CORNER_PREFERENCE) {
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const c_void,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

/// `radius`: radio de las esquinas en píxeles físicos; `None` vuelve a las nativas.
/// Hay que llamarla de nuevo cada vez que cambia el tamaño de la ventana.
pub fn set_corners(hwnd: isize, radius: Option<i32>) {
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as *mut c_void);
    match radius {
        Some(radius) => {
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                return;
            }
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let diameter = radius.max(0) * 2;
            set_corner_preference(hwnd, DWMWCP_DONOTROUND);
            unsafe {
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
                // La ventana se queda con la región: no hay que liberarla.
                SetWindowRgn(hwnd, Some(region), true);
            }
        }
        None => {
            unsafe {
                SetWindowRgn(hwnd, None::<HRGN>, true);
            }
            set_corner_preference(hwnd, DWMWCP_ROUND);
        }
    }
}

/// Área de trabajo del monitor donde está la ventana (todo el monitor, menos
/// la barra de tareas y demás barras acopladas), en píxeles físicos:
/// `(x, y, ancho, alto)` de esa área en coordenadas de pantalla. `None` si
/// falla (la app usa un tamaño por defecto).
pub fn work_area(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    if hwnd == 0 {
        return None;
    }
    unsafe {
        let monitor = MonitorFromWindow(HWND(hwnd as *mut c_void), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(monitor, &mut info).as_bool().then(|| {
            let area = info.rcWork;
            (area.left, area.top, area.right - area.left, area.bottom - area.top)
        })
    }
}

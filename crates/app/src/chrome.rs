//! Ventana sin marco: backend winit con la sombra y las esquinas redondeadas de
//! Windows 11, arrastre, minimizar/maximizar/cerrar, tamaño de la interfaz y
//! seguimiento del foco y del tema.

use std::cell::Cell;
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::platform::windows::{CornerPreference, IconExtWindows, WindowAttributesExtWindows};
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::Icon;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::{App, AppWindow, prefs};

/// Ícono embebido en el .exe (ver `build.rs`).
const ICON_RESOURCE: u16 = 1;
/// Radio de las esquinas estilo macOS, en px de la interfaz (coincide con el
/// borde que dibuja `app.slint`). Con `SetWindowRgn` los píxeles fuera de la
/// región no reciben clics (no hay transparencia real por píxel en este
/// renderer): un radio más chico deja más margen para agarrar justo la
/// esquina y redimensionar en diagonal.
const MAC_CORNER_RADIUS: f32 = 11.0;

thread_local! {
    /// El tamaño de la interfaz se aplica recién cuando existe la ventana real.
    static WINDOW_READY: Cell<bool> = const { Cell::new(false) };
}

/// Elige el backend winit. Dibuja con Skia por GPU (Direct3D) y, si la placa de
/// video no está disponible, Slint cae solo al renderizador por software.
/// `SLINT_BACKEND=winit-software` fuerza el respaldo para diagnosticar.
pub fn select_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(|attributes| {
            let icon = Icon::from_resource(ICON_RESOURCE, None).ok();
            attributes
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round)
                // Sin borde de DWM: la línea blanca pegada arriba de la ventana.
                .with_border_color(None)
                .with_window_icon(icon.clone())
                .with_taskbar_icon(icon)
                // Arranca oculta: se muestra recién cuando ya tiene el zoom y
                // las esquinas aplicados (ver `schedule_zoom`), para no
                // mostrar un primer cuadro sin ajustar (a veces se veía como
                // una franja en blanco arriba, del recuadro nativo todavía
                // sin la forma final).
                .with_visible(false)
        })
        .select()
}

/// HWND de la ventana (0 si todavía no existe), para diálogos nativos con dueño.
pub fn hwnd(app: &AppWindow) -> isize {
    app.window().with_winit_window(hwnd_of).unwrap_or(0)
}

fn hwnd_of(window: &slint::winit_030::winit::window::Window) -> isize {
    match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    }
}

/// Aplica el tamaño de la interfaz elegido en Ajustes de la app, sobre la escala
/// de Windows: todo (texto, íconos y medidas) crece o se achica junto.
pub fn apply_zoom(app: &AppWindow) {
    if !WINDOW_READY.get() {
        return;
    }
    let zoom = prefs::get().zoom;
    let Some(system) = app.window().with_winit_window(|window| window.scale_factor() as f32) else { return };
    app.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: system * zoom });
}

/// Tamaño "de diseño" (lo declarado en `app.slint`: `preferred-width/height` y
/// `min-width/height`), en píxeles lógicos al 100% de escala. Se repite acá
/// porque Slint no expone esas propiedades para leerlas desde Rust.
const PREFERRED_SIZE: (f32, f32) = (1140.0, 760.0);
const MIN_SIZE: (f32, f32) = (860.0, 540.0);
/// La ventana no ocupa el área de trabajo entera: deja un margen para que no
/// quede pegada a los bordes de la pantalla ni tape la barra de tareas.
const SCREEN_MARGIN: f32 = 0.92;

/// Ventana según la pantalla: el tamaño "de diseño" (1140×760) entra cómodo en
/// un monitor grande, pero con una pantalla chica, alta escala de Windows o
/// mucho "tamaño de la interfaz" ese mismo tamaño lógico puede pedir más
/// píxeles físicos de los que el monitor tiene, y quedar cortada. Achica hasta
/// el mínimo (860×540, ya soportado en toda la interfaz por sus áreas de
/// desplazamiento) para que siempre entre entera en el área de trabajo,
/// centrada; en un monitor grande simplemente usa el tamaño de diseño.
pub fn fit_to_screen(app: &AppWindow) {
    let zoom = prefs::get().zoom;
    app.window().with_winit_window(|window| {
        use slint::winit_030::winit::dpi::{PhysicalPosition, PhysicalSize};
        let system = window.scale_factor() as f32;
        let physical_scale = system * zoom;
        let Some((area_x, area_y, area_w, area_h)) = liquidcontrol_core::window::work_area(hwnd_of(window)) else {
            return;
        };

        let fit = |available: i32, min: f32, preferred: f32| -> f32 {
            (available as f32 * SCREEN_MARGIN / physical_scale).clamp(min, preferred)
        };
        let width = fit(area_w, MIN_SIZE.0, PREFERRED_SIZE.0) * physical_scale;
        let height = fit(area_h, MIN_SIZE.1, PREFERRED_SIZE.1) * physical_scale;
        let _ = window.request_inner_size(PhysicalSize::new(width.round() as u32, height.round() as u32));
        window.set_outer_position(PhysicalPosition::new(
            area_x + ((area_w as f32 - width) / 2.0).round() as i32,
            area_y + ((area_h as f32 - height) / 2.0).round() as i32,
        ));
    });
}

/// Esquinas de la ventana según Ajustes de la app: las nativas de Windows 11 o
/// más redondeadas (estilo macOS). Al maximizar se vuelve a la forma rectangular.
pub fn apply_corners(app: &AppWindow) {
    if !WINDOW_READY.get() {
        return;
    }
    let mac = prefs::get().window_corners == prefs::WindowCorners::Mac && !app.window().is_maximized();
    let scale = app.window().scale_factor();
    liquidcontrol_core::window::set_corners(hwnd(app), mac.then(|| (MAC_CORNER_RADIUS * scale).round() as i32));
    app.global::<App>().set_mac_corners(mac);
}

/// Aplica tamaño y esquinas en la próxima vuelta del bucle (no dentro de un evento de ventana).
fn schedule_zoom(app: &AppWindow) {
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(app) = weak.upgrade() {
            fit_to_screen(&app);
            apply_zoom(&app);
            apply_corners(&app);
            // Recién ahora se muestra: ya tiene el tamaño y la forma final,
            // sin el cuadro nativo sin ajustar de por medio.
            app.window().with_winit_window(|window| window.set_visible(true));
        }
    });
}

pub fn init(app: &AppWindow) {
    let global = app.global::<App>();

    let weak = app.as_weak();
    global.on_window_drag(move || {
        if let Some(app) = weak.upgrade() {
            app.window().with_winit_window(|window| {
                // Arrastre nativo: también activa Aero Snap.
                let _ = window.drag_window();
            });
        }
    });

    let weak = app.as_weak();
    global.on_window_minimize(move || {
        if let Some(app) = weak.upgrade() {
            app.window().set_minimized(true);
        }
    });

    let weak = app.as_weak();
    global.on_window_maximize(move || {
        if let Some(app) = weak.upgrade() {
            let maximized = !app.window().is_maximized();
            app.window().set_maximized(maximized);
            app.global::<App>().set_maximized(maximized);
        }
    });

    global.on_window_close(|| {
        let _ = slint::quit_event_loop();
    });

    let weak = app.as_weak();
    app.window().on_winit_window_event(move |window, event| {
        if let Some(app) = weak.upgrade() {
            match event {
                WindowEvent::Focused(focused) => app.global::<App>().set_window_active(*focused),
                // Cubre también Win+↑, Aero Snap y el doble clic en la barra.
                WindowEvent::Resized(_) => {
                    app.global::<App>().set_maximized(window.is_maximized());
                    if !WINDOW_READY.replace(true) {
                        schedule_zoom(&app);
                    } else {
                        // La región de las esquinas depende del tamaño: se rehace en cada cambio.
                        apply_corners(&app);
                    }
                }
                // Windows cambió la escala (otro monitor, otro DPI): se vuelve a sumar el zoom.
                WindowEvent::ScaleFactorChanged { .. } => schedule_zoom(&app),
                // Windows pasó a modo claro u oscuro: si el tema es "Sistema", seguirlo.
                WindowEvent::ThemeChanged(_) => prefs::apply(&app),
                _ => {}
            }
        }
        EventResult::Propagate
    });
}

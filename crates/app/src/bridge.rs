//! Trabajo en segundo plano. Las llamadas al sistema (registro, COM, WinRT…)
//! corren en un hilo aparte y el resultado vuelve al hilo de la interfaz, que
//! así nunca se congela.

use slint::ComponentHandle;

use crate::AppWindow;

/// Ejecuta `work` en un hilo aparte y entrega su resultado a `done` en el hilo de
/// la interfaz. Si la ventana ya se cerró, el resultado se descarta.
pub fn spawn<T, W, D>(app: &AppWindow, work: W, done: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(&AppWindow, T) + Send + 'static,
{
    let weak = app.as_weak();
    let spawned = std::thread::Builder::new().name("liquidcontrol-tarea".into()).spawn(move || {
        let result = work();
        // Solo falla si el bucle de eventos ya terminó (la app se está cerrando).
        let _ = weak.upgrade_in_event_loop(move |app| done(&app, result));
    });
    if let Err(err) = spawned {
        log::error!("no se pudo crear un hilo de trabajo: {err}");
    }
}

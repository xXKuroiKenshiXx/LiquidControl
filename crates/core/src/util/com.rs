//! COM aislado en hilos propios.
//!
//! Cada llamada COM corre en un hilo nuevo con el apartamento que necesita
//! (STA para objetos del shell, MTA para WinRT y Core Audio) y siempre se
//! desinicializa al terminar. Así el hilo de la interfaz nunca queda atado a COM.

use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};

use crate::error::{AppError, AppResult};

fn run_in_apartment<T, F>(sta: bool, work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    std::thread::spawn(move || {
        let flags = if sta { COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE } else { COINIT_MULTITHREADED };
        let init = unsafe { CoInitializeEx(None, flags) };
        let result = work();
        if init.is_ok() {
            unsafe { CoUninitialize() };
        }
        result
    })
    .join()
    .map_err(|_| AppError::failed("El hilo COM terminó inesperadamente"))?
}

/// Ejecuta `work` en un apartamento de un solo hilo (objetos del shell). Bloquea hasta terminar.
pub fn sta<T, F>(work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    run_in_apartment(true, work)
}

/// Ejecuta `work` en el apartamento multihilo (WinRT, Core Audio). Bloquea hasta terminar.
pub fn mta<T, F>(work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    run_in_apartment(false, work)
}

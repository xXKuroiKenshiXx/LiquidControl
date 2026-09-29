//! Portapapeles de Windows (texto Unicode).

use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

use crate::error::{AppError, AppResult};

/// Copia `text` al portapapeles.
pub fn set_text(text: &str) -> AppResult<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        OpenClipboard(None)?;
        let result = (|| -> AppResult<()> {
            EmptyClipboard()?;
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * size_of::<u16>())?;
            let target = GlobalLock(memory) as *mut u16;
            if target.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(AppError::failed("No se pudo preparar el portapapeles."));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
            let _ = GlobalUnlock(memory);
            // Si SetClipboardData funciona, la memoria pasa a ser del sistema.
            if let Err(err) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))) {
                let _ = GlobalFree(Some(memory));
                return Err(err.into());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

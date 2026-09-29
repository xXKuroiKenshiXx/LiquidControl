//! Datos de versión de un ejecutable, para mostrar su nombre comercial.

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::core::{HSTRING, w};

/// "Descripción del archivo" del ejecutable (p. ej. "Spotify" para `Spotify.exe`).
pub fn description(path: &Path) -> Option<String> {
    let file = HSTRING::from(path.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(&file, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&file, None, size, data.as_mut_ptr() as *mut c_void).ok()?;
        let block = data.as_ptr() as *const c_void;
        // Primer par idioma/página de códigos que declara el archivo.
        let (mut pointer, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
        if !VerQueryValueW(block, w!("\\VarFileInfo\\Translation"), &mut pointer, &mut len).as_bool() || len < 4 {
            return None;
        }
        let pair = pointer as *const u16;
        let (language, codepage) = (*pair, *pair.add(1));
        let query = HSTRING::from(format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription"));
        if !VerQueryValueW(block, &query, &mut pointer, &mut len).as_bool() || len == 0 {
            return None;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer as *const u16, len as usize));
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

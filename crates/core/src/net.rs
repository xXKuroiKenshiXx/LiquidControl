//! Descargas HTTPS pequeñas (por ejemplo, íconos del catálogo) con WinHTTP, el
//! cliente nativo de Windows: usa el proxy del sistema y valida TLS con Schannel.
//!
//! Solo HTTPS, solo hosts con nombre válido y con un tamaño máximo. Si no hay
//! conexión, devuelve un error y quien llama sigue sin internet.

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::error::{AppError, AppResult};

const TIMEOUT_MS: i32 = 6000;
// Descargas grandes: cada lectura es chica (64 KiB), así que el timeout sigue
// siendo "por operación", no total, pero se da más margen si la conexión es lenta.
const DOWNLOAD_TIMEOUT_MS: i32 = 20000;
const MAX_HOST_LEN: usize = 253;

/// Cierra el handle de WinHTTP al salir del alcance.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> AppResult<Self> {
        if raw.is_null() { Err(windows::core::Error::from_win32().into()) } else { Ok(Self(raw)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// Separa "https://host/ruta" en (host, "/ruta"). Rechaza todo lo que no sea HTTPS
/// o tenga un host con caracteres raros (sin usuario, puerto ni IP literal rara).
fn split_url(url: &str) -> AppResult<(&str, String)> {
    let rest = url.strip_prefix("https://").ok_or_else(|| AppError::invalid("Solo se permiten descargas HTTPS."))?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    let valid_host = !host.is_empty()
        && host.len() <= MAX_HOST_LEN
        && host.contains('.')
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !valid_host || path.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(AppError::invalid("Dirección de descarga inválida."));
    }
    Ok((host, path))
}

/// Descarga `url` completa si pesa como máximo `max_bytes`.
pub fn get(url: &str, max_bytes: usize) -> AppResult<Vec<u8>> {
    let (host, path) = split_url(url)?;
    let agent = HSTRING::from(format!("LiquidControl/{}", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session =
            Handle::new(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS)?;
        let connection = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;

        let (mut status, mut size, mut index) = (0u32, size_of::<u32>() as u32, 0u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            &mut index,
        )?;
        if status != 200 {
            // Código propio: quien llama distingue "el sitio respondió" de "no hay conexión".
            return Err(AppError::Coded("http_status", format!("El servidor respondió {status}.")));
        }

        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read)?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > max_bytes {
                return Err(AppError::failed("La descarga es más grande de lo permitido."));
            }
        }
        Ok(body)
    }
}

/// Descarga `url` a `dest` (con un `.part` intermedio, para no dejar un
/// archivo a medias si se corta). `on_progress(descargado, total)` se llama
/// después de cada lectura; `total` es 0 si el servidor no mandó el tamaño.
pub fn download_to_file(url: &str, dest: &Path, mut on_progress: impl FnMut(u64, u64)) -> AppResult<()> {
    let (host, path) = split_url(url)?;
    let agent = HSTRING::from(format!("LiquidControl/{}", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session =
            Handle::new(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(
            session.0,
            DOWNLOAD_TIMEOUT_MS,
            DOWNLOAD_TIMEOUT_MS,
            DOWNLOAD_TIMEOUT_MS,
            DOWNLOAD_TIMEOUT_MS,
        )?;
        let connection = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;

        let (mut status, mut size, mut index) = (0u32, size_of::<u32>() as u32, 0u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            &mut index,
        )?;
        if status != 200 {
            return Err(AppError::Coded("http_status", format!("El servidor respondió {status}.")));
        }

        let (mut content_length, mut cl_size, mut cl_index) = (0u32, size_of::<u32>() as u32, 0u32);
        // El tamaño total es solo informativo (barra de progreso): si el
        // servidor no lo manda, se sigue sin él.
        let _ = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut content_length as *mut u32 as *mut c_void),
            &mut cl_size,
            &mut cl_index,
        );

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part = dest.with_extension("part");
        let mut file = File::create(&part)?;
        let mut downloaded = 0u64;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read)?;
            if read == 0 {
                break;
            }
            file.write_all(&chunk[..read as usize])?;
            downloaded += u64::from(read);
            on_progress(downloaded, u64::from(content_length));
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, dest)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::split_url;

    #[test]
    fn only_https_and_clean_hosts() {
        assert_eq!(
            split_url("https://www.raycast.com/favicon.ico").unwrap(),
            ("www.raycast.com", "/favicon.ico".into())
        );
        assert_eq!(split_url("https://example.com").unwrap(), ("example.com", "/".into()));
        assert!(split_url("http://example.com/a").is_err());
        assert!(split_url("https://user@example.com/a").is_err());
        assert!(split_url("https://example.com:8443/a").is_err());
        assert!(split_url("https://example.com/a b").is_err());
        assert!(split_url("file:///C:/Windows").is_err());
    }
}

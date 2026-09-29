//! Página web de AirDrop para celulares que no tienen ninguna app instalada
//! (el caso típico: un iPhone). El usuario escanea el QR que muestra LiquidControl,
//! Safari abre esta página y desde ahí puede:
//!
//! - **Mandar a la PC**: la página usa las mismas rutas `prepare-upload` y
//!   `upload` de LocalSend, así que pasa por la misma aceptación del usuario y
//!   las mismas validaciones que cualquier otro equipo.
//! - **Bajar de la PC**: solo los archivos que el usuario compartió a
//!   propósito, y solo con la clave del QR.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{CHUNK, PORT, Request, Shared, lock, mime_for, respond, respond_with};
use crate::error::{AppError, AppResult};
use crate::media;
use crate::util::{com, shell};

const PAGE: &str = include_str!("web.html");
/// Cuántos archivos se pueden compartir a la vez con el celular.
const MAX_SHARES: usize = 50;

/// Dirección que va en el QR, o vacía si está apagado o no hay red.
pub(super) fn url(locked: &Shared) -> String {
    if locked.web_key.is_empty() {
        return String::new();
    }
    lan_ip().map_or_else(String::new, |ip| format!("http://{ip}:{PORT}/?k={}", locked.web_key))
}

/// IP de esta PC en la red local: la de la placa por donde saldría el tráfico.
/// `connect` en UDP no manda ningún paquete, solo elige la ruta.
fn lan_ip() -> Option<Ipv4Addr> {
    let targets = [
        SocketAddr::from(([8, 8, 8, 8], 80)),
        SocketAddr::from(([192, 168, 1, 1], 80)),
        SocketAddr::from(([10, 0, 0, 1], 80)),
        SocketAddr::from((super::MULTICAST, PORT)),
    ];
    targets.into_iter().find_map(|target| {
        let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
        socket.connect(target).ok()?;
        match socket.local_addr().ok()? {
            SocketAddr::V4(addr) if !addr.ip().is_unspecified() && !addr.ip().is_loopback() => Some(*addr.ip()),
            _ => None,
        }
    })
}

fn key_ok(request: &Request) -> bool {
    let locked = lock();
    !locked.web_key.is_empty() && request.query.get("k") == Some(&locked.web_key)
}

pub(super) fn page(stream: &mut TcpStream, request: &Request) {
    if !key_ok(request) {
        let body = "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
                    <body style='font-family:system-ui;padding:32px;text-align:center'>\
                    <h2>Código vencido</h2><p>Volvé a escanear el código QR que muestra LiquidControl en la PC.</p>";
        respond_with(stream, 403, "text/html; charset=utf-8", "", body.as_bytes());
        return;
    }
    let (key, alias) = {
        let locked = lock();
        (locked.web_key.clone(), locked.alias.clone())
    };
    let body = PAGE.replace("{{KEY}}", &key).replace("{{ALIAS}}", &escape_html(&alias));
    respond_with(stream, 200, "text/html; charset=utf-8", "Cache-Control: no-store\r\n", body.as_bytes());
}

#[derive(Serialize)]
struct WebFile {
    index: usize,
    name: String,
    size: u64,
}

pub(super) fn files(stream: &mut TcpStream, request: &Request) {
    if !key_ok(request) {
        respond(stream, 403, "[]");
        return;
    }
    let list: Vec<WebFile> = lock()
        .web_shares
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let meta = fs::metadata(path).ok()?;
            Some(WebFile { index, name: file_name(path), size: meta.len() })
        })
        .collect();
    respond(stream, 200, &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
}

pub(super) fn download(stream: &mut TcpStream, request: &Request) {
    if !key_ok(request) {
        respond(stream, 403, "{}");
        return;
    }
    let path =
        request.query.get("i").and_then(|i| i.parse::<usize>().ok()).and_then(|i| lock().web_shares.get(i).cloned());
    let Some(path) = path else {
        respond(stream, 404, "{}");
        return;
    };
    let (Ok(mut file), Ok(meta)) = (File::open(&path), fs::metadata(&path)) else {
        respond(stream, 404, "{}");
        return;
    };
    let name = file_name(&path);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename*=UTF-8''{}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        mime_for(&path),
        meta.len(),
        encode(&name)
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    // En pedazos: un video grande no se carga entero en RAM.
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if stream.write_all(&buffer[..read]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = stream.flush();
    log::info!(target: "airdrop", "El celular bajó «{name}»");
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archivo".to_owned())
}

/// Para `filename*=` (RFC 5987): todo lo que no sea letra, número o `-_.~` va codificado.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

// ─────────────────────────── API pública ───────────────────────────

/// Suma archivos a los que el celular puede bajar (sin repetir).
pub fn share(paths: Vec<PathBuf>) -> AppResult<()> {
    let mut locked = lock();
    if locked.web_key.is_empty() {
        return Err(AppError::invalid("Prendé AirDrop primero."));
    }
    for path in paths {
        if path.is_file() && !locked.web_shares.contains(&path) && locked.web_shares.len() < MAX_SHARES {
            locked.web_shares.push(path);
        }
    }
    Ok(())
}

/// Deja de compartir un archivo (por su posición en la lista).
pub fn unshare(index: usize) {
    let mut locked = lock();
    if index < locked.web_shares.len() {
        locked.web_shares.remove(index);
    }
}

/// Los últimos archivos recibidos en `dir`, del más nuevo al más viejo.
pub fn received(dir: &Path, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), entry.path()))
        })
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.into_iter().take(limit).map(|(_, path)| path).collect()
}

/// Abre un archivo recibido. Las imágenes se abren con el visor; cualquier
/// otra cosa solo se muestra seleccionada en el Explorador, así un `.exe` o
/// un acceso directo que mandó otro equipo nunca se ejecuta con un clic.
pub fn open_received(dir: &Path, path: &Path) -> AppResult<()> {
    let (Ok(dir), Ok(path)) = (dir.canonicalize(), path.canonicalize()) else {
        return Err(AppError::not_found("El archivo ya no está."));
    };
    if !path.starts_with(&dir) || !path.is_file() {
        return Err(AppError::invalid("Ese archivo no está en la carpeta de AirDrop."));
    }
    let target = path.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    if media::is_image(&path) {
        com::sta(move || shell::open(&target))
    } else {
        shell::launch("explorer.exe", &format!("/select,\"{target}\""))
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, escape_html};

    #[test]
    fn download_names_are_encoded() {
        assert_eq!(encode("foto 1.jpg"), "foto%201.jpg");
        assert_eq!(encode("año\".txt"), "a%C3%B1o%22.txt");
    }

    #[test]
    fn alias_cannot_inject_html() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
    }
}

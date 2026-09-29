//! AirDrop: enviar y recibir archivos con otros equipos de la red local,
//! hablando el protocolo abierto de **LocalSend v2**
//! (<https://github.com/localsend/protocol>).
//!
//! Es una implementación propia en Rust de esa especificación pública: no se
//! copió código de la app LocalSend (Dart). Al hablar el mismo protocolo,
//! LiquidControl se ve y transfiere con cualquier dispositivo que tenga LocalSend
//! (Windows, Android, iOS, Linux, macOS) sin instalar nada más.
//!
//! Piezas:
//! 1. **Descubrimiento**: anuncios JSON por UDP multicast a 224.0.0.167:53317.
//! 2. **Servidor HTTP** en el puerto 53317: `info`, `register`,
//!    `prepare-upload`, `upload` y `cancel`.
//! 3. **Cliente HTTP** para mandar archivos al equipo elegido.
//! 4. **Página web** para celulares sin ninguna app (por ejemplo un iPhone):
//!    se abre escaneando un código QR y deja mandar fotos y archivos a la PC o
//!    bajar los que la PC comparta. Usa las mismas rutas de subida (y la misma
//!    aceptación del usuario) que LocalSend.
//!
//! Sobre la seguridad (esto abre un puerto en la red local):
//! - Arranca **apagado**; solo escucha mientras el usuario lo prende.
//! - **Nada se guarda sin aceptar**: cada envío entrante espera la decisión
//!   del usuario (y se rechaza solo si no contesta).
//! - Los nombres de archivo entrantes se sanean a fondo (ver
//!   [`safe_file_name`]): nunca se escribe fuera de la carpeta elegida.
//! - Cada archivo necesita el token de su sesión y que la IP coincida con la
//!   del equipo que pidió la transferencia.
//! - La página web y las descargas piden una clave al azar que viaja en el QR
//!   y cambia cada vez que se prende AirDrop. Solo se puede bajar lo que el
//!   usuario compartió a propósito.
//! - Va en HTTP plano (el protocolo lo contempla y LocalSend lo permite con
//!   el cifrado apagado): no se manejan certificados ni claves.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

mod web;

pub use web::{open_received, received, share as share_with_web, unshare as unshare_web};

const MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
const PORT: u16 = 53317;
const PROTOCOL_VERSION: &str = "2.0";
/// Cada cuánto se avisa "acá estoy" al grupo multicast.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(5);
/// Un equipo que no vuelve a anunciarse en este tiempo se saca de la lista.
const PEER_TTL: Duration = Duration::from_secs(30);
/// Lo que se espera a que el usuario acepte o rechace un envío entrante.
const DECISION_TIMEOUT: Duration = Duration::from_secs(60);
/// Pedazo para la página web y lecturas chicas.
const CHUNK: usize = 64 * 1024;
/// Pedazo para copiar archivos: con 1 MB se hacen muchas menos llamadas al
/// sistema por gigabyte que con 64 KB (medido con `examples/bench_airdrop`).
const FILE_CHUNK: usize = 1024 * 1024;
/// Archivos que se mandan a la vez: muchas fotos chicas viajan mucho más
/// rápido en paralelo que de a una.
const PARALLEL_UPLOADS: usize = 4;
/// Mientras una transferencia está en pausa la conexión queda quieta: el
/// tiempo de espera tiene que dar margen para pausas largas.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Techo por archivo entrante (evita llenar el disco por accidente).
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;

// ─────────────────────────── Tipos del protocolo ───────────────────────────

/// Anuncio/registro de un equipo (el mismo objeto que viaja por UDP y por
/// `register`; los campos coinciden con la especificación de LocalSend).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    alias: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    device_model: Option<String>,
    #[serde(default)]
    device_type: Option<String>,
    fingerprint: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    download: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    announce: Option<bool>,
}

/// Del archivo anunciado solo hacen falta el nombre y el tamaño: el id es la
/// clave del mapa (que es lo que el otro equipo va a usar al subirlo) y el
/// resto de los campos del protocolo se ignoran.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDto {
    file_name: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct PrepareUploadDto {
    #[serde(default)]
    info: Option<DeviceInfo>,
    files: HashMap<String, FileDto>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareUploadResponse {
    session_id: String,
    files: HashMap<String, String>,
}

// ─────────────────────────── Estado para la interfaz ───────────────────────────

/// Un equipo visible en la red local.
#[derive(Debug, Clone, Serialize)]
pub struct Peer {
    pub fingerprint: String,
    pub alias: String,
    pub model: String,
    /// "mobile", "desktop", "web", "headless" o "server".
    pub kind: String,
    pub ip: String,
    pub port: u16,
}

/// Un archivo de un envío entrante.
#[derive(Debug, Clone, Serialize)]
pub struct IncomingFile {
    pub id: String,
    pub name: String,
    pub size: u64,
}

/// Envío entrante esperando que el usuario lo acepte o lo rechace.
#[derive(Debug, Clone, Serialize)]
pub struct IncomingRequest {
    pub session_id: String,
    pub sender: String,
    pub files: Vec<IncomingFile>,
    pub total_bytes: u64,
}

/// Qué está pasando ahora mismo con una transferencia.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Activity {
    #[default]
    Idle,
    Receiving,
    Sending,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub alias: String,
    pub peers: Vec<Peer>,
    pub pending: Option<IncomingRequest>,
    pub activity: Activity,
    /// Bytes transferidos y totales de la transferencia en curso.
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// La transferencia en curso está en pausa.
    pub paused: bool,
    /// Última carpeta donde se guardó algo (para el botón "Abrir carpeta").
    pub last_folder: String,
    /// Dirección para abrir desde el celular (vacía si está apagado o no hay red).
    pub web_url: String,
    /// Archivos que la PC ofrece para bajar desde la página web.
    pub web_shares: Vec<SharedFile>,
}

/// Un archivo que la PC ofrece para bajar desde el celular.
#[derive(Debug, Clone, Serialize)]
pub struct SharedFile {
    pub name: String,
    pub size: u64,
}

// ─────────────────────────── Estado interno ───────────────────────────

#[derive(Default)]
struct Session {
    id: String,
    /// `fileId` → (token, nombre ya saneado, tamaño declarado).
    files: HashMap<String, (String, String, u64)>,
    from: Option<IpAddr>,
    dir: PathBuf,
    received: usize,
}

#[derive(Default)]
struct Shared {
    alias: String,
    fingerprint: String,
    peers: HashMap<String, (Peer, Instant)>,
    /// Envío entrante esperando decisión y la decisión tomada (`None` = sin
    /// responder todavía).
    pending: Option<IncomingRequest>,
    decision: Option<bool>,
    accept_dir: PathBuf,
    session: Option<Session>,
    activity: Activity,
    last_folder: String,
    /// Clave de la página web (va en el QR; cambia en cada encendido).
    web_key: String,
    web_shares: Vec<PathBuf>,
}

struct Global {
    shared: Mutex<Shared>,
    decided: Condvar,
}

static GLOBAL: LazyLock<Global> =
    LazyLock::new(|| Global { shared: Mutex::new(Shared::default()), decided: Condvar::new() });
static RUNNING: AtomicBool = AtomicBool::new(false);
static DONE_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);
/// El usuario pausó la transferencia en curso (se deja de leer o de escribir
/// y TCP frena al otro equipo solo, sin perder nada).
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Cambia con cada cancelación: una transferencia que empezó con otro valor
/// se corta.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Espera mientras esté en pausa. `false` = se canceló o se apagó AirDrop.
fn keep_going(epoch: u64) -> bool {
    loop {
        if EPOCH.load(Ordering::SeqCst) != epoch || !RUNNING.load(Ordering::SeqCst) {
            return false;
        }
        if !PAUSED.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn lock() -> std::sync::MutexGuard<'static, Shared> {
    GLOBAL.shared.lock().unwrap_or_else(|poison| poison.into_inner())
}

// ─────────────────────────── Utilidades ───────────────────────────

/// Número pseudoaleatorio simple: alcanza para ids de sesión y tokens, que
/// solo tienen que ser difíciles de adivinar dentro de la red local y duran
/// lo que dura una transferencia.
fn random_hex(bytes: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
        ^ (COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        ^ (std::process::id() as u64) << 32;
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes {
        // xorshift64*
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let value = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.push_str(&format!("{:02x}", (value >> 33) as u8));
    }
    out
}

/// Nombre de archivo seguro para guardar algo que mandó otro equipo.
///
/// Se queda solo con el último tramo de la ruta y saca todo lo que podría
/// escribir fuera de la carpeta elegida o romper en Windows: separadores,
/// `..`, caracteres prohibidos, nombres reservados (`CON`, `LPT1`…) y puntos
/// o espacios al final. Devuelve `None` si no queda nada usable.
pub fn safe_file_name(raw: &str) -> Option<String> {
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1",
        "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String =
        last.chars()
            .map(|c| {
                if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    let stem = cleaned.split('.').next().unwrap_or(cleaned).to_lowercase();
    let cleaned = if RESERVED.contains(&stem.as_str()) { format!("_{cleaned}") } else { cleaned.to_owned() };
    // Windows admite 255, pero se corta antes para dejar lugar al sufijo de
    // "ya existe" y a la carpeta.
    Some(cleaned.chars().take(120).collect())
}

/// Ruta libre dentro de `dir`: si ya existe, agrega " (2)", " (3)"…
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem} ({}){ext}", random_hex(4)))
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn me(locked: &Shared) -> DeviceInfo {
    DeviceInfo {
        alias: locked.alias.clone(),
        version: PROTOCOL_VERSION.to_owned(),
        device_model: Some("Windows".to_owned()),
        device_type: Some("desktop".to_owned()),
        fingerprint: locked.fingerprint.clone(),
        port: Some(PORT),
        protocol: Some("http".to_owned()),
        download: Some(false),
        announce: None,
    }
}

// ─────────────────────────── HTTP mínimo ───────────────────────────

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    length: usize,
    /// `Transfer-Encoding: chunked`: el cuerpo llega en pedazos sin largo total.
    /// Es lo que usa LocalSend (su cliente HTTP en Rust) al subir archivos.
    chunked: bool,
}

/// Cuerpo de un pedido, con largo fijo o en pedazos ("chunked").
struct Body<'a> {
    reader: &'a mut BufReader<TcpStream>,
    chunked: bool,
    /// Largo fijo: lo que falta leer. Chunked: lo que falta del pedazo actual.
    left: u64,
    done: bool,
}

impl<'a> Body<'a> {
    fn new(reader: &'a mut BufReader<TcpStream>, chunked: bool, length: usize) -> Self {
        Self { reader, chunked, left: if chunked { 0 } else { length as u64 }, done: false }
    }

    /// Una línea corta (tamaño de pedazo o encabezado final): nunca más de 256 bytes.
    fn line(&mut self) -> std::io::Result<String> {
        let mut line = String::new();
        self.reader.by_ref().take(256).read_line(&mut line)?;
        Ok(line)
    }
}

impl Read for Body<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Error, ErrorKind};
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if !self.chunked {
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
            let want = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
            let read = self.reader.read(&mut buf[..want])?;
            if read == 0 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "se cortó el cuerpo"));
            }
            self.left -= read as u64;
            return Ok(read);
        }
        if self.left == 0 {
            let line = self.line()?;
            let size = line.trim().split(';').next().unwrap_or_default().trim();
            let size = u64::from_str_radix(size, 16)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "tamaño de pedazo inválido"))?;
            if size == 0 {
                // Encabezados finales opcionales hasta la línea vacía.
                while !self.line()?.trim().is_empty() {}
                self.done = true;
                return Ok(0);
            }
            self.left = size;
        }
        let want = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let read = self.reader.read(&mut buf[..want])?;
        if read == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "se cortó el pedazo"));
        }
        self.left -= read as u64;
        if self.left == 0 {
            let mut crlf = [0u8; 2];
            self.reader.read_exact(&mut crlf)?;
        }
        Ok(read)
    }
}

fn parse_request(reader: &mut BufReader<TcpStream>) -> AppResult<Request> {
    let mut line = String::new();
    let mut read = 0;
    if reader.read_line(&mut line)? == 0 {
        return Err(AppError::failed("Conexión cerrada."));
    }
    read += line.len();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let (path, raw_query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let query = raw_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let path = path.to_owned();

    let mut length = 0usize;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        read += header.len();
        if read > MAX_HEADER_BYTES {
            return Err(AppError::invalid("Encabezados demasiado largos."));
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.trim().eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.to_ascii_lowercase().contains("chunked");
            }
        }
    }
    Ok(Request { method, path, query, length, chunked })
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    respond_with(stream, status, "application/json", "", body.as_bytes());
}

fn respond_with(stream: &mut TcpStream, status: u16, content_type: &str, extra_headers: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

// ─────────────────────────── Servidor ───────────────────────────

fn handle(stream: TcpStream) {
    let peer_ip = stream.peer_addr().ok().map(|addr| addr.ip());
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut stream = stream;
    let Ok(request) = parse_request(&mut reader) else {
        respond(&mut stream, 400, "{}");
        return;
    };

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/localsend/v2/info") | ("POST", "/api/localsend/v2/register") => {
            // En `register` el otro equipo se presenta: se lo agrega a la lista.
            if request.length > 0 || request.chunked {
                if let Ok(body) = read_json(&mut reader, &request) {
                    if let Ok(info) = serde_json::from_slice::<DeviceInfo>(&body) {
                        remember(info, peer_ip);
                    }
                }
            }
            let info = me(&lock());
            respond(&mut stream, 200, &serde_json::to_string(&info).unwrap_or_else(|_| "{}".into()));
        }
        ("POST", "/api/localsend/v2/prepare-upload") => prepare_upload(&mut reader, &mut stream, &request, peer_ip),
        ("POST", "/api/localsend/v2/upload") => upload(&mut reader, &mut stream, &request, peer_ip),
        ("GET", "/") => web::page(&mut stream, &request),
        ("GET", "/web/files") => web::files(&mut stream, &request),
        ("GET", "/web/download") => web::download(&mut stream, &request),
        ("POST", "/api/localsend/v2/cancel") => {
            let mut locked = lock();
            locked.session = None;
            locked.activity = Activity::Idle;
            drop(locked);
            respond(&mut stream, 200, "{}");
        }
        _ => respond(&mut stream, 404, "{}"),
    }
}

fn read_json(reader: &mut BufReader<TcpStream>, request: &Request) -> AppResult<Vec<u8>> {
    if request.length > MAX_JSON_BYTES {
        return Err(AppError::invalid("Cuerpo demasiado grande."));
    }
    let mut body = Vec::with_capacity(request.length);
    Body::new(reader, request.chunked, request.length).take(MAX_JSON_BYTES as u64 + 1).read_to_end(&mut body)?;
    if body.len() > MAX_JSON_BYTES {
        return Err(AppError::invalid("Cuerpo demasiado grande."));
    }
    Ok(body)
}

fn prepare_upload(reader: &mut BufReader<TcpStream>, stream: &mut TcpStream, request: &Request, from: Option<IpAddr>) {
    let Ok(body) = read_json(reader, request) else {
        respond(stream, 400, "{}");
        return;
    };
    let Ok(dto) = serde_json::from_slice::<PrepareUploadDto>(&body) else {
        respond(stream, 400, "{}");
        return;
    };
    if dto.files.is_empty() {
        respond(stream, 204, "{}");
        return;
    }

    let sender = dto.info.as_ref().map_or_else(|| "Equipo desconocido".to_owned(), |info| info.alias.clone());
    // Un navegador no tiene servidor propio: no se lo lista como equipo.
    if let Some(info) = dto.info.clone().filter(|info| info.device_type.as_deref() != Some("web")) {
        remember(info, from);
    }

    let session_id = random_hex(8);
    let mut files = HashMap::new();
    let mut listed = Vec::new();
    let mut total = 0u64;
    for (id, file) in &dto.files {
        let Some(name) = safe_file_name(&file.file_name) else { continue };
        if file.size > MAX_FILE_BYTES {
            continue;
        }
        total += file.size;
        listed.push(IncomingFile { id: id.clone(), name: name.clone(), size: file.size });
        files.insert(id.clone(), (random_hex(8), name, file.size));
    }
    if listed.is_empty() {
        respond(stream, 400, "{}");
        return;
    }

    // Se le pregunta al usuario y se espera su decisión.
    let mut locked = lock();
    if locked.session.is_some() || locked.pending.is_some() {
        drop(locked);
        respond(stream, 409, "{}");
        return;
    }
    listed.sort_by(|a, b| a.name.cmp(&b.name));
    locked.pending =
        Some(IncomingRequest { session_id: session_id.clone(), sender, files: listed, total_bytes: total });
    locked.decision = None;
    let (mut locked, timeout) = GLOBAL
        .decided
        .wait_timeout_while(locked, DECISION_TIMEOUT, |shared| {
            shared.decision.is_none() && RUNNING.load(Ordering::SeqCst)
        })
        .unwrap_or_else(|poison| poison.into_inner());
    let accepted = locked.decision.take().unwrap_or(false) && !timeout.timed_out();
    locked.pending = None;
    if !accepted {
        drop(locked);
        respond(stream, 403, "{}");
        return;
    }

    let dir = locked.accept_dir.clone();
    let tokens: HashMap<String, String> = files.iter().map(|(id, (token, ..))| (id.clone(), token.clone())).collect();
    locked.session = Some(Session { id: session_id.clone(), files, from, dir, received: 0 });
    locked.activity = Activity::Receiving;
    drop(locked);
    PAUSED.store(false, Ordering::SeqCst);
    DONE_BYTES.store(0, Ordering::SeqCst);
    TOTAL_BYTES.store(total, Ordering::SeqCst);

    let response = PrepareUploadResponse { session_id, files: tokens };
    respond(stream, 200, &serde_json::to_string(&response).unwrap_or_else(|_| "{}".into()));
}

fn upload(reader: &mut BufReader<TcpStream>, stream: &mut TcpStream, request: &Request, from: Option<IpAddr>) {
    let (Some(session_id), Some(file_id), Some(token)) =
        (request.query.get("sessionId"), request.query.get("fileId"), request.query.get("token"))
    else {
        respond(stream, 400, "{}");
        return;
    };

    // Se valida contra la sesión aceptada: id, token, equipo y tamaño.
    let target = {
        let locked = lock();
        let Some(session) = locked.session.as_ref() else {
            respond(stream, 409, "{}");
            return;
        };
        if &session.id != session_id || (session.from.is_some() && session.from != from) {
            respond(stream, 403, "{}");
            return;
        }
        match session.files.get(file_id) {
            Some((expected, name, size)) if expected == token => (session.dir.clone(), name.clone(), *size),
            _ => {
                respond(stream, 403, "{}");
                return;
            }
        }
    };
    let (dir, name, declared) = target;
    if !request.chunked && request.length as u64 != declared {
        respond(stream, 400, "{}");
        return;
    }

    if fs::create_dir_all(&dir).is_err() {
        respond(stream, 500, "{}");
        return;
    }
    let path = unique_path(&dir, &name);
    let Ok(mut file) = File::create(&path) else {
        respond(stream, 500, "{}");
        return;
    };
    // Reservar el tamaño de una vez evita que el disco agrande el archivo en
    // cada escritura (y que quede fragmentado).
    let _ = file.set_len(declared);
    let _ = stream.set_read_timeout(Some(TRANSFER_TIMEOUT));
    let epoch = EPOCH.load(Ordering::SeqCst);

    // Se copia en pedazos: un archivo grande no puede quedar entero en RAM.
    // Nunca se acepta más de lo declarado, y un archivo incompleto se borra:
    // es preferible un error a quedarse con una foto vacía o cortada.
    let mut body = Body::new(reader, request.chunked, request.length);
    let mut written = 0u64;
    let mut buffer = vec![0u8; FILE_CHUNK];
    let complete = loop {
        if !keep_going(epoch) {
            break false;
        }
        match body.read(&mut buffer) {
            Ok(0) => break written == declared,
            Ok(read) => {
                written += read as u64;
                if written > declared || file.write_all(&buffer[..read]).is_err() {
                    break false;
                }
                DONE_BYTES.fetch_add(read as u64, Ordering::Relaxed);
            }
            Err(_) => break false,
        }
    };
    let flushed = file.flush().is_ok();
    // Se cierra antes de contestar: "listo" tiene que querer decir que el
    // archivo ya está cerrado y completo en el disco.
    drop(file);
    if !complete || !flushed {
        let _ = fs::remove_file(&path);
        log::warn!(target: "airdrop", "«{name}» llegó incompleto ({written} de {declared} bytes): se descartó");
        // La transferencia se da por perdida: si no, la sesión quedaría
        // tomada y cualquier envío siguiente respondería "ocupado".
        let mut locked = lock();
        locked.session = None;
        locked.activity = Activity::Idle;
        drop(locked);
        respond(stream, 400, "{}");
        return;
    }
    log::info!(target: "airdrop", "Se recibió «{name}» ({written} bytes)");

    let mut locked = lock();
    let finished = match locked.session.as_mut() {
        Some(session) => {
            session.received += 1;
            session.received >= session.files.len()
        }
        None => true,
    };
    if finished {
        locked.session = None;
        locked.activity = Activity::Idle;
        locked.last_folder = dir.to_string_lossy().into_owned();
    }
    drop(locked);
    respond(stream, 200, "{}");
}

fn remember(info: DeviceInfo, from: Option<IpAddr>) {
    let mut locked = lock();
    if info.fingerprint == locked.fingerprint || info.fingerprint.is_empty() {
        return;
    }
    let Some(ip) = from else { return };
    let peer = Peer {
        fingerprint: info.fingerprint.clone(),
        alias: info.alias.clone(),
        model: info.device_model.clone().unwrap_or_default(),
        kind: info.device_type.clone().unwrap_or_else(|| "desktop".to_owned()),
        ip: ip.to_string(),
        port: info.port.unwrap_or(PORT),
    };
    locked.peers.insert(info.fingerprint, (peer, Instant::now()));
}

// ─────────────────────────── Descubrimiento ───────────────────────────

fn announce_payload(announce: bool) -> String {
    let mut info = me(&lock());
    info.announce = Some(announce);
    serde_json::to_string(&info).unwrap_or_default()
}

fn discovery_loop(socket: UdpSocket) {
    let mut buffer = vec![0u8; 8 * 1024];
    let mut last = Instant::now() - ANNOUNCE_EVERY;
    while RUNNING.load(Ordering::SeqCst) {
        if last.elapsed() >= ANNOUNCE_EVERY {
            let payload = announce_payload(true);
            let _ = socket.send_to(payload.as_bytes(), SocketAddr::from((MULTICAST, PORT)));
            last = Instant::now();
            // De paso, se limpian los que dejaron de anunciarse.
            lock().peers.retain(|_, (_, seen)| seen.elapsed() < PEER_TTL);
        }
        match socket.recv_from(&mut buffer) {
            Ok((read, from)) => {
                let Ok(info) = serde_json::from_slice::<DeviceInfo>(&buffer[..read]) else { continue };
                let announced = info.announce.unwrap_or(false);
                let is_me = info.fingerprint == lock().fingerprint;
                remember(info, Some(from.ip()));
                // Si el otro se está presentando, se le contesta directo.
                if announced && !is_me {
                    let _ = socket.send_to(announce_payload(false).as_bytes(), from);
                }
            }
            // Sin novedades en este rato: se vuelve a mirar si hay que anunciarse.
            Err(_) => continue,
        }
    }
}

// ─────────────────────────── API pública ───────────────────────────

/// Prende AirDrop: abre el puerto, empieza a anunciarse y a escuchar.
/// `alias` es el nombre con el que los demás equipos nos van a ver.
pub fn start(alias: &str, accept_dir: PathBuf) -> AppResult<()> {
    if RUNNING.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Si se acaba de apagar, los hilos anteriores tardan hasta medio segundo en
    // soltar el puerto: se reintenta un momento antes de dar error.
    let listener = retry_bind(|| TcpListener::bind(("0.0.0.0", PORT))).map_err(|err| {
        AppError::failed(format!(
            "No se pudo abrir el puerto {PORT} ({err}). Si tenés LocalSend abierto, cerralo y probá de nuevo."
        ))
    })?;

    let socket = retry_bind(|| UdpSocket::bind(("0.0.0.0", PORT)))
        .map_err(|err| AppError::failed(format!("No se pudo escuchar la red local en el puerto {PORT} ({err}).")))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    socket.set_multicast_loop_v4(false)?;
    // Si falla el "join" igual se puede recibir por unicast y mandar por
    // multicast: no se aborta por eso.
    let _ = socket.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED);

    {
        let mut locked = lock();
        locked.alias = alias.to_owned();
        if locked.fingerprint.is_empty() {
            locked.fingerprint = random_hex(16);
        }
        locked.accept_dir = accept_dir;
        locked.web_key = random_hex(12);
        locked.peers.clear();
        locked.activity = Activity::Idle;
    }
    RUNNING.store(true, Ordering::SeqCst);

    std::thread::Builder::new().name("liquidcontrol-airdrop-udp".into()).spawn(move || discovery_loop(socket))?;
    std::thread::Builder::new().name("liquidcontrol-airdrop-tcp".into()).spawn(move || {
        // `accept` bloqueante: cada conexión se atiende al instante. Antes se
        // revisaba cada 80 ms y cada archivo esperaba eso antes de empezar
        // (con 100 fotos, segundos perdidos). `stop` lo destraba conectándose.
        for stream in listener.incoming() {
            if !RUNNING.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let _ = std::thread::Builder::new().name("liquidcontrol-airdrop-conn".into()).spawn(move || handle(stream));
        }
    })?;
    log::info!(target: "airdrop", "AirDrop encendido como «{alias}»");
    Ok(())
}

fn retry_bind<T>(bind: impl Fn() -> std::io::Result<T>) -> std::io::Result<T> {
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        match bind() {
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            result => return result,
        }
    }
}

/// Apaga AirDrop: deja de escuchar y de anunciarse.
pub fn stop() {
    if !RUNNING.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut locked = lock();
    locked.peers.clear();
    locked.session = None;
    locked.pending = None;
    locked.decision = Some(false);
    locked.activity = Activity::Idle;
    locked.web_key.clear();
    locked.web_shares.clear();
    drop(locked);
    PAUSED.store(false, Ordering::SeqCst);
    GLOBAL.decided.notify_all();
    // Destraba el `accept` del hilo del servidor para que suelte el puerto.
    let _ = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], PORT)), Duration::from_millis(300));
    log::info!(target: "airdrop", "AirDrop apagado");
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Estado completo para la interfaz (se pide cada pocos segundos).
pub fn status() -> Status {
    let locked = lock();
    let mut peers: Vec<Peer> = locked.peers.values().map(|(peer, _)| peer.clone()).collect();
    peers.sort_by(|a, b| a.alias.cmp(&b.alias));
    Status {
        running: RUNNING.load(Ordering::SeqCst),
        alias: locked.alias.clone(),
        peers,
        pending: locked.pending.clone(),
        activity: locked.activity,
        done_bytes: DONE_BYTES.load(Ordering::Relaxed),
        total_bytes: TOTAL_BYTES.load(Ordering::Relaxed),
        paused: PAUSED.load(Ordering::SeqCst),
        last_folder: locked.last_folder.clone(),
        web_url: web::url(&locked),
        web_shares: locked
            .web_shares
            .iter()
            .map(|path| SharedFile {
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                size: fs::metadata(path).map_or(0, |meta| meta.len()),
            })
            .collect(),
    }
}

/// Pausa o reanuda la transferencia en curso (recibir o enviar).
pub fn set_paused(paused: bool) {
    PAUSED.store(paused, Ordering::SeqCst);
    log::info!(target: "airdrop", "Transferencia {}", if paused { "en pausa" } else { "reanudada" });
}

/// Corta la transferencia en curso. Lo que no llegó completo se borra.
pub fn cancel_transfer() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    let mut locked = lock();
    if locked.activity == Activity::Receiving {
        locked.session = None;
        locked.activity = Activity::Idle;
    }
    drop(locked);
    log::info!(target: "airdrop", "Transferencia cancelada");
}

/// Acepta o rechaza el envío entrante que está esperando.
pub fn decide(accept: bool) {
    let mut locked = lock();
    if locked.pending.is_some() {
        locked.decision = Some(accept);
    }
    drop(locked);
    GLOBAL.decided.notify_all();
}

/// Manda archivos a un equipo de la lista.
pub fn send(peer: &Peer, paths: &[PathBuf]) -> AppResult<()> {
    if paths.is_empty() {
        return Err(AppError::invalid("No hay archivos para enviar."));
    }
    let ip: IpAddr = peer.ip.parse().map_err(|_| AppError::invalid("Dirección del equipo inválida."))?;
    let addr = SocketAddr::new(ip, peer.port);

    // 1. Se avisa qué se va a mandar y se esperan los tokens.
    let mut entries = serde_json::Map::new();
    let mut total = 0u64;
    for (index, path) in paths.iter().enumerate() {
        let meta = fs::metadata(path)?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("archivo");
        total += meta.len();
        entries.insert(
            format!("f{index}"),
            serde_json::json!({
                "id": format!("f{index}"),
                "fileName": name,
                "size": meta.len(),
                "fileType": mime_for(path),
            }),
        );
    }
    let info = me(&lock());
    let body = serde_json::json!({ "info": info, "files": entries }).to_string();

    DONE_BYTES.store(0, Ordering::SeqCst);
    TOTAL_BYTES.store(total, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    lock().activity = Activity::Sending;
    let result = send_all(addr, &body, paths, EPOCH.load(Ordering::SeqCst));
    lock().activity = Activity::Idle;
    PAUSED.store(false, Ordering::SeqCst);
    result
}

fn send_all(addr: SocketAddr, prepare_body: &str, paths: &[PathBuf], epoch: u64) -> AppResult<()> {
    let (status, response) =
        post(addr, "/api/localsend/v2/prepare-upload", "application/json", prepare_body.as_bytes())?;
    match status {
        200 => {}
        403 => return Err(AppError::failed("El otro equipo rechazó el envío.")),
        409 => return Err(AppError::failed("El otro equipo está ocupado con otra transferencia.")),
        _ => return Err(AppError::failed(format!("El otro equipo respondió {status}."))),
    }
    let prepared: PrepareUploadResponse = serde_json::from_slice(&response)
        .map_err(|_| AppError::failed("El otro equipo contestó algo que no se entiende."))?;

    // Varios archivos a la vez; si uno falla o el usuario cancela, paran todos.
    let queue: Mutex<Vec<(String, &PathBuf)>> = Mutex::new(
        paths
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(index, path)| {
                let id = format!("f{index}");
                let token = prepared.files.get(&id)?;
                Some((
                    format!("/api/localsend/v2/upload?sessionId={}&fileId={id}&token={token}", prepared.session_id),
                    path,
                ))
            })
            .collect(),
    );
    let failed = AtomicBool::new(false);
    let first_error: Mutex<Option<AppError>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_UPLOADS.min(paths.len()) {
            scope.spawn(|| {
                loop {
                    if failed.load(Ordering::SeqCst) {
                        return;
                    }
                    let Some((url, path)) = queue.lock().unwrap_or_else(|p| p.into_inner()).pop() else { return };
                    let go_on = || !failed.load(Ordering::SeqCst) && keep_going_sending(epoch);
                    match post_file(addr, &url, path, &go_on) {
                        Ok(()) => log::info!(target: "airdrop", "Se envió «{}»", path.display()),
                        Err(err) => {
                            failed.store(true, Ordering::SeqCst);
                            first_error.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(err);
                            return;
                        }
                    }
                }
            });
        }
    });
    if EPOCH.load(Ordering::SeqCst) != epoch {
        // Se avisa al otro equipo para que no quede esperando el resto.
        let _ =
            post(addr, &format!("/api/localsend/v2/cancel?sessionId={}", prepared.session_id), "application/json", b"");
        return Err(AppError::failed("Cancelaste el envío."));
    }
    match first_error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Como [`keep_going`], pero para enviar: se puede mandar aunque AirDrop no
/// esté escuchando.
fn keep_going_sending(epoch: u64) -> bool {
    loop {
        if EPOCH.load(Ordering::SeqCst) != epoch {
            return false;
        }
        if !PAUSED.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn connect(addr: SocketAddr) -> AppResult<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|err| AppError::failed(format!("No se pudo conectar con el equipo ({err}).")))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(60)))?;
    Ok(stream)
}

fn read_response(stream: TcpStream) -> AppResult<(u16, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
    let mut length = 0usize;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.trim().eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.to_ascii_lowercase().contains("chunked");
            }
        }
    }
    let mut body = Vec::new();
    Body::new(&mut reader, chunked, length.min(MAX_JSON_BYTES)).take(MAX_JSON_BYTES as u64).read_to_end(&mut body)?;
    Ok((status, body))
}

fn post(addr: SocketAddr, path: &str, content_type: &str, body: &[u8]) -> AppResult<(u16, Vec<u8>)> {
    let mut stream = connect(addr)?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    read_response(stream)
}

fn post_file(addr: SocketAddr, path: &str, file: &Path, go_on: &dyn Fn() -> bool) -> AppResult<()> {
    let mut source = File::open(file)?;
    let length = source.metadata()?.len();
    let mut stream = connect(addr)?;
    stream.set_write_timeout(Some(TRANSFER_TIMEOUT))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/octet-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    let mut buffer = vec![0u8; FILE_CHUNK];
    loop {
        if !go_on() {
            return Err(AppError::failed("Se canceló el envío."));
        }
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        stream.write_all(&buffer[..read])?;
        DONE_BYTES.fetch_add(read as u64, Ordering::Relaxed);
    }
    stream.flush()?;
    let (status, _) = read_response(stream)?;
    if status != 200 {
        return Err(AppError::failed(format!("El otro equipo respondió {status} al recibir el archivo.")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{safe_file_name, unique_path};

    #[test]
    fn file_names_cannot_escape_the_folder() {
        assert_eq!(safe_file_name("foto.png").as_deref(), Some("foto.png"));
        assert_eq!(safe_file_name(r"..\..\Windows\System32\evil.dll").as_deref(), Some("evil.dll"));
        assert_eq!(safe_file_name("/etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_file_name(r"C:\Windows\hosts").as_deref(), Some("hosts"));
        assert_eq!(safe_file_name("..").as_deref(), None);
        assert_eq!(safe_file_name("   ").as_deref(), None);
        assert_eq!(safe_file_name("").as_deref(), None);
        // Nombres reservados de Windows y caracteres prohibidos.
        assert_eq!(safe_file_name("CON").as_deref(), Some("_CON"));
        assert_eq!(safe_file_name("lpt1.txt").as_deref(), Some("_lpt1.txt"));
        assert_eq!(safe_file_name("a:b?c*.txt").as_deref(), Some("a_b_c_.txt"));
        assert!(!safe_file_name("nombre\u{0}raro.txt").unwrap().contains('\u{0}'));
        // Punto final: Windows lo ignora, así que se saca.
        assert_eq!(safe_file_name("archivo.").as_deref(), Some("archivo"));
        assert!(safe_file_name(&"a".repeat(400)).unwrap().chars().count() <= 120);
    }

    #[test]
    fn unique_path_keeps_the_extension() {
        let dir = std::env::temp_dir();
        let path = unique_path(&dir, "no-existe-seguro-12345.txt");
        assert_eq!(path.file_name().unwrap(), "no-existe-seguro-12345.txt");
    }
}

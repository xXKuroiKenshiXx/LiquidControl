//! AirDrop: encender/apagar, ver los equipos de la red, aceptar o rechazar lo
//! que llega, mandar archivos, el QR para celulares sin app y la vista previa
//! de lo recibido. Todo el protocolo vive en `liquidcontrol_core::airdrop`; acá solo
//! se refleja su estado en la interfaz.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use liquidcontrol_core::airdrop::{self, Activity, IncomingRequest};
use liquidcontrol_core::{actions, media, picker};
use qrcode::{Color, EcLevel, QrCode};
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

use super::{Pane, fixed_target, fixed_title};
use crate::images::{self, Source};
use crate::toasts::{self, Kind};
use crate::{
    AirdropFileView, AirdropPeerView, AirdropReceivedView, AirdropState, AppWindow, Nav, bridge, chrome, format, nav,
};

pub const PANE: Pane = Pane::new("airdrop", |app, _| enter(app))
    .with_init(init)
    .with_leave(|app, _| leave(app))
    .with_subpages(sub_title, target);

/// Subpágina con el código QR para el iPhone y celulares sin app.
const SUBPAGES: [(&str, &str); 1] = [("phone", "iPhone y celulares")];

fn sub_title(sub: &str) -> Option<String> {
    fixed_title(&SUBPAGES, sub)
}

fn target(item: &str) -> (String, String) {
    fixed_target(&SUBPAGES, item)
}

/// Cuántos archivos recibidos se muestran con vista previa.
const RECEIVED_LIMIT: usize = 12;
const PREVIEW_SIZE: u32 = 200;

/// Descarta miniaturas de una carga anterior o de un panel que ya se cerró.
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Mientras AirDrop está prendido se consulta seguido: los equipos aparecen y
/// desaparecen, y un pedido entrante tiene que verse enseguida.
const POLL: Duration = Duration::from_millis(700);

thread_local! {
    static TIMER: Timer = Timer::default();
    /// Id del último pedido entrante que ya se avisó (para no repetir el aviso).
    static NOTIFIED: RefCell<String> = RefCell::default();
    /// Dirección del QR que se está mostrando (se regenera solo si cambia).
    static QR_URL: RefCell<String> = RefCell::default();
    /// Archivos de la grilla de recibidos, en el mismo orden.
    static RECEIVED: RefCell<Vec<PathBuf>> = RefCell::default();
    /// Última muestra para calcular la velocidad: (momento, bytes, MB/s suavizados).
    static SPEED: Cell<Option<(Instant, u64, f64)>> = const { Cell::new(None) };
    /// Actividad del sondeo anterior: al terminar de recibir se refresca la grilla.
    static LAST_ACTIVITY: Cell<i32> = const { Cell::new(0) };
}

fn init(app: &AppWindow) {
    let state = app.global::<AirdropState>();

    let weak = app.as_weak();
    state.on_share_web(move || {
        let Some(app) = weak.upgrade() else { return };
        let owner = chrome::hwnd(&app);
        bridge::spawn(
            &app,
            move || picker::pick_files(owner).and_then(airdrop::share_with_web),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
                show(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_unshare_web(move |index| {
        let Some(app) = weak.upgrade() else { return };
        airdrop::unshare_web(usize::try_from(index).unwrap_or(usize::MAX));
        show(&app);
    });

    let weak = app.as_weak();
    state.on_open_received(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(path) = RECEIVED.with_borrow(|list| usize::try_from(index).ok().and_then(|i| list.get(i).cloned()))
        else {
            return;
        };
        bridge::spawn(
            &app,
            move || airdrop::open_received(&save_folder(), &path),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load_received(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_running(move |on| {
        let Some(app) = weak.upgrade() else { return };
        let state = app.global::<AirdropState>();
        state.set_busy(true);
        state.set_running(on);
        if on {
            let alias = alias();
            let folder = save_folder();
            bridge::spawn(
                &app,
                move || airdrop::start(&alias, folder),
                |app, result| {
                    app.global::<AirdropState>().set_busy(false);
                    if let Err(err) = result {
                        app.global::<AirdropState>().set_running(false);
                        toasts::error(&err);
                    }
                    show(app);
                    start_polling(app);
                },
            );
        } else {
            bridge::spawn(&app, airdrop::stop, |app, ()| {
                app.global::<AirdropState>().set_busy(false);
                TIMER.with(Timer::stop);
                show(app);
            });
        }
    });

    let weak = app.as_weak();
    state.on_accept(move || {
        if let Some(app) = weak.upgrade() {
            bridge::spawn(&app, || airdrop::decide(true), |app, ()| show(app));
        }
    });

    let weak = app.as_weak();
    state.on_set_paused(move |paused| {
        let Some(app) = weak.upgrade() else { return };
        airdrop::set_paused(paused);
        show(&app);
    });

    let weak = app.as_weak();
    state.on_cancel_transfer(move || {
        let Some(app) = weak.upgrade() else { return };
        airdrop::cancel_transfer();
        toasts::show(Kind::Info, "Transferencia cancelada. Lo que no llegó completo se borró.");
        show(&app);
    });

    let weak = app.as_weak();
    state.on_reject(move || {
        if let Some(app) = weak.upgrade() {
            bridge::spawn(&app, || airdrop::decide(false), |app, ()| show(app));
        }
    });

    let weak = app.as_weak();
    state.on_send_to(move |fingerprint| {
        let Some(app) = weak.upgrade() else { return };
        send_to(&app, fingerprint.to_string());
    });

    let weak = app.as_weak();
    state.on_open_folder(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(
            &app,
            move || {
                let folder = save_folder();
                std::fs::create_dir_all(&folder)?;
                actions::open_folder(&folder)
            },
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });
}

/// Nombre con el que nos ven los demás equipos: el del equipo en Windows.
fn alias() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|name| !name.is_empty()).unwrap_or_else(|| "Windows".to_owned())
}

/// Carpeta donde se guarda lo que llega: `Descargas\LiquidControl`.
fn save_folder() -> PathBuf {
    let base = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Downloads").join("LiquidControl")
}

fn send_to(app: &AppWindow, fingerprint: String) {
    let Some(peer) = airdrop::status().peers.into_iter().find(|peer| peer.fingerprint == fingerprint) else {
        toasts::show(Kind::Info, "Ese equipo ya no está a la vista.");
        return;
    };
    let owner = chrome::hwnd(app);
    let alias = peer.alias.clone();
    bridge::spawn(
        app,
        move || picker::pick_files(owner).map(|paths| (peer, paths)),
        move |app, result| match result {
            Ok((_, paths)) if paths.is_empty() => {}
            Ok((peer, paths)) => {
                let count = paths.len();
                toasts::show(Kind::Info, format!("Enviando {count} archivo(s) a «{alias}»…"));
                start_polling(app);
                bridge::spawn(
                    app,
                    move || airdrop::send(&peer, &paths),
                    move |app, result| {
                        match result {
                            Ok(()) => toasts::show(Kind::Success, "Listo, se enviaron los archivos."),
                            Err(err) => toasts::error(&err),
                        }
                        show(app);
                    },
                );
            }
            Err(err) => toasts::error(&err),
        },
    );
}

fn enter(app: &AppWindow) {
    show(app);
    load_received(app);
}

/// Al salir se sueltan las miniaturas (el sondeo sigue si está prendido).
fn leave(app: &AppWindow) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    RECEIVED.with_borrow_mut(Vec::clear);
    app.global::<AirdropState>().set_received(ModelRc::default());
}

/// Lista lo último recibido y carga sus vistas previas en segundo plano.
fn load_received(app: &AppWindow) {
    let folder = save_folder();
    bridge::spawn(
        app,
        move || airdrop::received(&folder, RECEIVED_LIMIT),
        |app, paths| {
            if app.global::<Nav>().get_pane() != "airdrop" {
                return;
            }
            let views: Vec<AirdropReceivedView> = paths
                .iter()
                .map(|path| AirdropReceivedView {
                    name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into(),
                    image: Image::default(),
                    loaded: false,
                })
                .collect();
            let state = app.global::<AirdropState>();
            state.set_received(ModelRc::new(VecModel::from(views)));
            state.set_has_heic(paths.iter().any(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif"))
            }));
            // Solo las fotos tienen miniatura. El resto usa el ícono de archivo de
            // la app: los de Windows vienen sin transparencia para muchos tipos, y
            // así tampoco se lee el ícono de un ejecutable que mandó otro equipo.
            let jobs = paths
                .iter()
                .enumerate()
                .filter(|(_, path)| media::is_image(path))
                .map(|(index, path)| (index, Source::Photo(path.clone())))
                .collect();
            RECEIVED.set(paths);
            images::load(app, jobs, PREVIEW_SIZE, &GENERATION, set_preview);
        },
    );
}

fn set_preview(app: &AppWindow, index: usize, pixels: images::Pixels) {
    let items = app.global::<AirdropState>().get_received();
    if let Some(mut item) = items.row_data(index) {
        item.image = images::image(pixels);
        item.loaded = true;
        items.set_row_data(index, item);
    }
}

/// Código QR en negro sobre blanco, con su margen, a 4 px por módulo.
fn qr_image(url: &str) -> Image {
    const QUIET: usize = 2;
    const SCALE: usize = 4;
    let Ok(code) = QrCode::with_error_correction_level(url, EcLevel::M) else { return Image::default() };
    let modules = code.width();
    let colors = code.to_colors();
    let side = (modules + QUIET * 2) * SCALE;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(side as u32, side as u32);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i % side) / SCALE, (i / side) / SCALE);
        let dark = (QUIET..modules + QUIET).contains(&x)
            && (QUIET..modules + QUIET).contains(&y)
            && colors[(y - QUIET) * modules + (x - QUIET)] == Color::Dark;
        let value = if dark { 0 } else { 255 };
        *pixel = Rgba8Pixel::new(value, value, value, 255);
    }
    Image::from_rgba8(buffer)
}

/// Al entrar al panel (y mientras esté prendido) se refresca solo.
fn show(app: &AppWindow) {
    let status = airdrop::status();
    let state = app.global::<AirdropState>();
    state.set_running(status.running);
    state.set_alias(status.alias.as_str().into());
    state.set_folder(save_folder().to_string_lossy().as_ref().into());

    state.set_web_url(status.web_url.as_str().into());
    if QR_URL.with_borrow(|shown| *shown != status.web_url) {
        state.set_qr(if status.web_url.is_empty() { Image::default() } else { qr_image(&status.web_url) });
        QR_URL.set(status.web_url.clone());
    }
    let shares: Vec<AirdropFileView> = status
        .web_shares
        .iter()
        .map(|file| AirdropFileView { name: file.name.as_str().into(), size: format::bytes(file.size).into() })
        .collect();
    state.set_web_shares(ModelRc::new(VecModel::from(shares)));

    let peers: Vec<AirdropPeerView> = status
        .peers
        .iter()
        .map(|peer| AirdropPeerView {
            fingerprint: peer.fingerprint.as_str().into(),
            alias: peer.alias.as_str().into(),
            detail: if peer.model.is_empty() {
                peer.ip.as_str().into()
            } else {
                format!("{} · {}", peer.model, peer.ip).into()
            },
            kind: i32::from(peer.kind == "mobile"),
        })
        .collect();
    state.set_peers(ModelRc::new(VecModel::from(peers)));

    match &status.pending {
        Some(pending) => {
            let files: Vec<AirdropFileView> = pending
                .files
                .iter()
                .map(|file| AirdropFileView { name: file.name.as_str().into(), size: format::bytes(file.size).into() })
                .collect();
            state.set_has_pending(true);
            state.set_pending_sender(pending.sender.as_str().into());
            state.set_pending_summary(
                format!("{} archivo(s) · {}", pending.files.len(), format::bytes(pending.total_bytes)).into(),
            );
            state.set_pending_files(ModelRc::new(VecModel::from(files)));
            notify(app, pending);
        }
        None => {
            state.set_has_pending(false);
            state.set_pending_files(ModelRc::default());
            NOTIFIED.set(String::new());
        }
    }

    let activity = match status.activity {
        Activity::Idle => 0,
        Activity::Receiving => 1,
        Activity::Sending => 2,
    };
    state.set_activity(activity);
    // Terminó de recibir: la grilla de recibidos se pone al día.
    if LAST_ACTIVITY.replace(activity) == 1 && activity != 1 && app.global::<Nav>().get_pane() == "airdrop" {
        load_received(app);
    }
    let (done, total) = (status.done_bytes, status.total_bytes);
    state.set_paused(status.paused);
    state.set_progress(if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 });
    let rate = speed(activity != 0 && !status.paused, done);
    let mut label = format!("{} de {}", format::bytes(done), format::bytes(total));
    if rate > 0.0 {
        let left = total.saturating_sub(done) as f64 / rate;
        label.push_str(&format!(" · {}/s · {}", format::bytes(rate as u64), remaining(left)));
    }
    state.set_progress_label(label.into());
}

/// Bytes por segundo de la transferencia en curso, suavizados para que el
/// número no salte en cada sondeo. 0 si no hay nada o está en pausa.
fn speed(active: bool, done: u64) -> f64 {
    if !active {
        SPEED.set(None);
        return 0.0;
    }
    let now = Instant::now();
    match SPEED.get() {
        Some((at, before, smoothed)) if done >= before => {
            let elapsed = now.duration_since(at).as_secs_f64();
            if elapsed < 0.2 {
                return smoothed;
            }
            let current = (done - before) as f64 / elapsed;
            let smoothed = if smoothed > 0.0 { smoothed * 0.6 + current * 0.4 } else { current };
            SPEED.set(Some((now, done, smoothed)));
            smoothed
        }
        _ => {
            SPEED.set(Some((now, done, 0.0)));
            0.0
        }
    }
}

/// «quedan 12 s», «quedan 3 min»…
fn remaining(seconds: f64) -> String {
    match seconds {
        s if s < 1.0 => "casi listo".to_owned(),
        s if s < 60.0 => format!("quedan {} s", s.ceil() as u64),
        s if s < 3600.0 => format!("quedan {} min", (s / 60.0).ceil() as u64),
        s => format!("quedan {:.1} h", s / 3600.0),
    }
}

/// Si llega un pedido mientras el usuario está en otro panel, se avisa con un
/// aviso que lleva hasta acá.
fn notify(app: &AppWindow, pending: &IncomingRequest) {
    if app.global::<Nav>().get_pane() == "airdrop" {
        return;
    }
    if NOTIFIED.with_borrow(|id| *id == pending.session_id) {
        return;
    }
    NOTIFIED.set(pending.session_id.clone());
    let sender = pending.sender.clone();
    toasts::show_with_action(Kind::Info, format!("«{sender}» te quiere mandar archivos."), "Ver", |app| {
        nav::open(app, "airdrop", "", "");
    });
}

/// El sondeo sigue aunque el usuario se vaya del panel: si no, no se enteraría
/// de que alguien le quiere mandar algo.
fn start_polling(app: &AppWindow) {
    let weak = app.as_weak();
    TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, POLL, move || {
            let Some(app) = weak.upgrade() else { return };
            if !airdrop::is_running() {
                TIMER.with(Timer::stop);
            }
            show(&app);
        });
    });
}

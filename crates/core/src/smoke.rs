//! Pruebas de humo **de solo lectura** contra el Windows real. No cambian nada
//! del sistema; verifican que cada módulo funciona en la PC donde se corren:
//!
//! ```text
//! cargo test -p liquidcontrol-core -- --ignored --nocapture
//! ```

use serde::Serialize;

use crate::error::AppResult;
use crate::{
    appearance, apps, bluetooth, displays, icloud, input, media, network, notifications, power, sound, system, tweaks,
    wallpaper, wifi,
};

/// Imprime un resumen del resultado y falla la prueba si hubo error.
fn show<T: Serialize>(name: &str, result: AppResult<T>) -> serde_json::Value {
    match result {
        Ok(value) => {
            let json = serde_json::to_value(&value).expect("serializable");
            let text = json.to_string();
            println!("OK {name} ({} bytes): {}", text.len(), text.chars().take(600).collect::<String>());
            json
        }
        Err(err) => panic!("FALLÓ {name}: {err} [{}]", err.code()),
    }
}

#[test]
#[ignore]
fn sistema() {
    show("system::info", system::info());
    show("system::storage", Ok(system::storage()));
    show("system::account", Ok(system::account()));
    show("system::startup_items", Ok(system::startup_items()));
}

#[test]
#[ignore]
fn apariencia_y_entrada() {
    show("appearance::get", Ok(appearance::get()));
    show("input::mouse", Ok(input::mouse()));
    show("input::keyboard", Ok(input::keyboard()));
}

#[test]
#[ignore]
fn motor_de_ajustes() {
    for category in ["privacy", "desktop", "optimize", "accessibility"] {
        let list = tweaks::list(category).expect("lista de ajustes");
        let count = |state| list.iter().filter(|t| t.state == state).count();
        println!(
            "   {category}: {} ajustes -> {} activos, {} parciales, {} inactivos",
            list.len(),
            count(tweaks::TweakState::On),
            count(tweaks::TweakState::Partial),
            count(tweaks::TweakState::Off)
        );
    }
}

#[test]
#[ignore]
fn fondo_y_miniaturas() {
    show("wallpaper::current", wallpaper::current());
    let gallery = wallpaper::gallery();
    println!("   imágenes en la galería: {}", gallery.len());
    if let Some(image) = gallery.first() {
        let thumb = media::thumbnail(&image.path, 240).expect("miniatura");
        println!("   miniatura: {}x{} ({} bytes RGBA)", thumb.width, thumb.height, thumb.rgba.len());
        assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
    }
}

#[test]
#[ignore]
fn energia_sonido_notificaciones() {
    show("power::status", power::status());
    show("sound::status", sound::status());
    let state = notifications::get().expect("notificaciones");
    println!("   apps con notificaciones: {}", state.apps.len());
}

#[test]
#[ignore]
fn redes() {
    let status = show("wifi::status", wifi::status());
    if status["adapter"].is_string() && status["auto_config"] == true && status["location_denied"] == false {
        show("wifi::scan", wifi::scan());
    }
    show("network::adapters", network::adapters());
    show("bluetooth::status", bluetooth::status());
}

#[test]
#[ignore]
fn pantallas() {
    show("displays::list", Ok(displays::list()));
}

#[test]
#[ignore]
fn aplicaciones_e_icloud() {
    println!("   programas instalados: {}", apps::installed().len());
    let appx = apps::appx_list().expect("appx");
    println!("   apps preinstaladas presentes: {}", appx.iter().filter(|p| p.installed).count());
    show("icloud::status", Ok(icloud::status()));
}

#[test]
#[ignore]
fn winget() {
    println!("   paquetes winget detectados: {}", apps::winget_installed().expect("winget").len());
}

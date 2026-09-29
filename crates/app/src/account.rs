//! Tarjeta de la cuenta en la barra lateral: nombre, tipo de cuenta y foto.

use liquidcontrol_core::{media, system};
use slint::ComponentHandle;

use crate::{AccountState, App, AppWindow, bridge, images};

const AVATAR_SIZE: u32 = 96;

pub fn load(app: &AppWindow) {
    // Mientras se lee la cuenta, el nombre de usuario evita una tarjeta vacía.
    let username = std::env::var("USERNAME").unwrap_or_default();
    let global = app.global::<App>();
    global.set_initials(initials(&username).into());
    global.set_user_name(username.into());

    bridge::spawn(
        app,
        || {
            let account = system::account();
            let picture = account
                .picture_path
                .as_deref()
                .and_then(|path| media::thumbnail(path, AVATAR_SIZE).ok())
                .map(|thumb| images::pixels(&thumb));
            (account, picture)
        },
        |app, (account, picture)| {
            let details = app.global::<AccountState>();
            details.set_user_name(account.user_name.as_str().into());
            details.set_email(account.email.as_deref().unwrap_or_default().into());
            details.set_microsoft(account.microsoft_account);
            let global = app.global::<App>();
            global.set_user_name(account.display_name.as_str().into());
            global.set_initials(initials(&account.display_name).into());
            global.set_account_kind(if account.microsoft_account { "Cuenta Microsoft" } else { "Cuenta local" }.into());
            if let Some(pixels) = picture {
                global.set_avatar(images::image(pixels));
                global.set_has_avatar(true);
            }
        },
    );
}

/// Iniciales para el avatar sin foto: "Ana Pérez" → "AP".
fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|word| word.chars().next()).take(2).flat_map(char::to_uppercase).collect()
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn two_letters_max() {
        assert_eq!(initials("ana maría pérez"), "AM");
        assert_eq!(initials("kuro"), "K");
        assert_eq!(initials(""), "");
    }
}

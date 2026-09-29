//! Acceso al registro con rutas legibles ("HKCU\Software\...").
//! Siempre usa la vista de 64 bits para leer y escribir las mismas claves que Windows.
//! Cada escritura queda anotada en la consola de actividad (área "registro").

use std::io::ErrorKind;

use winreg::RegKey;
use winreg::enums::{
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, KEY_WRITE,
};

use crate::error::{AppError, AppResult};

/// Largo máximo de un texto en la consola (el registro puede guardar textos enormes).
const MAX_SHOWN: usize = 160;

/// Separa "HKCU\Ruta\Sub" en (raíz, "Ruta\Sub").
pub fn split(path: &str) -> AppResult<(RegKey, &str)> {
    let (hive, rest) = path.split_once('\\').unwrap_or((path, ""));
    let root = match hive {
        "HKCU" | "HKEY_CURRENT_USER" => RegKey::predef(HKEY_CURRENT_USER),
        "HKLM" | "HKEY_LOCAL_MACHINE" => RegKey::predef(HKEY_LOCAL_MACHINE),
        "HKCR" | "HKEY_CLASSES_ROOT" => RegKey::predef(HKEY_CLASSES_ROOT),
        _ => return Err(AppError::invalid(format!("Ruta de registro no soportada: {path}"))),
    };
    Ok((root, rest))
}

pub fn is_machine(path: &str) -> bool {
    path.starts_with("HKLM") || path.starts_with("HKEY_LOCAL_MACHINE")
}

pub fn open_read(path: &str) -> Option<RegKey> {
    let (root, sub) = split(path).ok()?;
    root.open_subkey_with_flags(sub, KEY_READ | KEY_WOW64_64KEY).ok()
}

pub fn key_exists(path: &str) -> bool {
    open_read(path).is_some()
}

pub fn read_dword(path: &str, name: &str) -> Option<u32> {
    open_read(path)?.get_value::<u32, _>(name).ok()
}

pub fn read_qword(path: &str, name: &str) -> Option<u64> {
    open_read(path)?.get_value::<u64, _>(name).ok()
}

pub fn read_string(path: &str, name: &str) -> Option<String> {
    open_read(path)?.get_value::<String, _>(name).ok()
}

pub fn read_binary(path: &str, name: &str) -> Option<Vec<u8>> {
    let value = open_read(path)?.get_raw_value(name).ok()?;
    Some(value.bytes.to_vec())
}

/// "Clave\Valor" para la consola ("(predeterminado)" si el nombre está vacío).
fn shown(path: &str, name: &str) -> String {
    if name.is_empty() { format!("{path}\\(predeterminado)") } else { format!("{path}\\{name}") }
}

fn create(path: &str) -> AppResult<RegKey> {
    let (root, sub) = split(path)?;
    let (key, _) = root.create_subkey_with_flags(sub, KEY_WRITE | KEY_WOW64_64KEY)?;
    Ok(key)
}

pub fn create_key(path: &str) -> AppResult<()> {
    create(path)?;
    log::info!(target: "registro", "Se creó la clave {path}");
    Ok(())
}

pub fn write_dword(path: &str, name: &str, value: u32) -> AppResult<()> {
    create(path)?.set_value(name, &value)?;
    log::info!(target: "registro", "{} = {value} (DWORD)", shown(path, name));
    Ok(())
}

pub fn write_qword(path: &str, name: &str, value: u64) -> AppResult<()> {
    create(path)?.set_value(name, &value)?;
    log::info!(target: "registro", "{} = {value} (QWORD)", shown(path, name));
    Ok(())
}

pub fn write_string(path: &str, name: &str, value: &str) -> AppResult<()> {
    create(path)?.set_value(name, &value)?;
    let text: String = value.chars().take(MAX_SHOWN).collect();
    log::info!(target: "registro", "{} = \"{text}\"", shown(path, name));
    Ok(())
}

pub fn write_binary(path: &str, name: &str, bytes: &[u8]) -> AppResult<()> {
    let value = winreg::RegValue { bytes: bytes.to_vec(), vtype: winreg::enums::RegType::REG_BINARY };
    create(path)?.set_raw_value(name, &value)?;
    log::info!(target: "registro", "{} = {} bytes (binario)", shown(path, name), bytes.len());
    Ok(())
}

/// Borra un valor. Que no exista (ni la clave ni el valor) no es un error.
pub fn delete_value(path: &str, name: &str) -> AppResult<()> {
    let (root, sub) = split(path)?;
    let key = match root.open_subkey_with_flags(sub, KEY_SET_VALUE | KEY_WOW64_64KEY) {
        Ok(key) => key,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    match key.delete_value(name) {
        Ok(()) => {
            log::info!(target: "registro", "Se borró {}", shown(path, name));
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Borra una clave con todo su contenido. Que no exista no es un error.
pub fn delete_tree(path: &str) -> AppResult<()> {
    let (root, sub) = split(path)?;
    if sub.is_empty() {
        return Err(AppError::invalid("No se puede borrar una raíz del registro"));
    }
    match root.delete_subkey_all(sub) {
        Ok(()) => {
            log::info!(target: "registro", "Se borró la clave {path} con todo su contenido");
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Nombres de las subclaves directas.
pub fn subkeys(path: &str) -> Vec<String> {
    open_read(path).map(|key| key.enum_keys().filter_map(Result::ok).collect()).unwrap_or_default()
}

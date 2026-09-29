//! Ejecución de procesos del sistema **sin ventana de consola**: nunca aparece
//! un CMD aparte; el comando y su resultado quedan en la consola de actividad.
//! Nunca se usa una shell intermedia: los argumentos viajan separados.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output};

use base64::Engine;

use crate::error::{AppError, AppResult};
use crate::util::win;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn run(program: &Path, args: &[&str], shown_args: &str) -> AppResult<Output> {
    let name = program.file_name().map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned());
    log::info!(target: "procesos", "Ejecutando {name} {shown_args}");
    let output = Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output()?;
    match output.status.code() {
        Some(0) => log::info!(target: "procesos", "{name} terminó bien"),
        code => log::warn!(
            target: "procesos",
            "{name} terminó con código {}: {}",
            code.map_or_else(|| "?".to_owned(), |c| c.to_string()),
            summary(&output)
        ),
    }
    Ok(output)
}

/// Ejecuta un programa oculto y espera a que termine.
pub fn run_hidden(program: &Path, args: &[&str]) -> AppResult<Output> {
    run(program, args, &args.join(" "))
}

/// Primera línea útil de stderr (o stdout), para mensajes de error legibles.
pub fn summary(output: &Output) -> String {
    let text = if output.stderr.is_empty() { &output.stdout } else { &output.stderr };
    String::from_utf8_lossy(text)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("sin detalles")
        .chars()
        .take(300)
        .collect()
}

/// Ejecuta un script de PowerShell **fijo** (embebido en el binario). Se pasa
/// codificado en Base64 (UTF-16LE) con `-EncodedCommand`, así no hay problemas
/// de comillas ni de interpretación de argumentos. `label` describe el script
/// en la consola de actividad.
///
/// Sin una consola real (el proceso corre oculto, `CREATE_NO_WINDOW`), la
/// primera vez que un script carga un módulo (p. ej. Defender) PowerShell
/// escribe avisos de progreso ("Preparando módulos para el primer uso...")
/// que, sin consola a la que mandarlos, terminan mezclados con la salida
/// estándar y rompen el parseo de scripts que devuelven JSON. Se desactivan
/// para todos los scripts.
pub fn powershell(label: &str, script: &str) -> AppResult<String> {
    let script = format!("$ProgressPreference='SilentlyContinue';$WarningPreference='SilentlyContinue';{script}");
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let exe = win::system32(r"WindowsPowerShell\v1.0\powershell.exe");
    let output = run(
        &exe,
        &["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded],
        &format!("(script interno: {label})"),
    )?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(AppError::failed(format!("PowerShell: {}", summary(&output))))
    }
}

/// Expande variables de entorno estilo `%SystemRoot%` (las desconocidas quedan igual).
pub fn expand_env(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push('%');
            rest = after;
            break;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Ejecutable de una línea de comandos del registro, si existe en el disco.
pub fn command_path(command: &str) -> Option<std::path::PathBuf> {
    let (exe, _) = split_command(command);
    let path = std::path::PathBuf::from(expand_env(&exe));
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// Separa una línea de comandos del registro en (ejecutable, argumentos).
pub fn split_command(command: &str) -> (String, String) {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_owned(), rest[end + 1..].trim().to_owned());
        }
    }
    let lower = command.to_ascii_lowercase();
    if let Some(pos) = lower.find(".exe") {
        let end = pos + 4;
        return (command[..end].to_owned(), command[end..].trim().to_owned());
    }
    (command.to_owned(), String::new())
}

#[cfg(test)]
mod tests {
    use super::{expand_env, split_command};

    #[test]
    fn expands_environment_variables() {
        let root = std::env::var("SystemRoot").unwrap();
        assert_eq!(expand_env(r"%SystemRoot%\notepad.exe"), format!(r"{root}\notepad.exe"));
        assert_eq!(expand_env("%NO_EXISTE_AJUSTES%\\x"), "%NO_EXISTE_AJUSTES%\\x");
        assert_eq!(expand_env("100%"), "100%");
    }

    #[test]
    fn splits_registry_command_lines() {
        assert_eq!(
            split_command(r#""C:\Program Files\App\unins000.exe" /SILENT"#),
            (r"C:\Program Files\App\unins000.exe".to_owned(), "/SILENT".to_owned())
        );
        assert_eq!(
            split_command(r"C:\Tools\app.exe --minimized"),
            (r"C:\Tools\app.exe".to_owned(), "--minimized".to_owned())
        );
        assert_eq!(split_command("MsiExec.exe /X{ABC}"), ("MsiExec.exe".to_owned(), "/X{ABC}".to_owned()));
    }
}

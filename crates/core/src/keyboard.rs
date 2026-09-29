//! Idiomas de escritura y distribuciones de teclado del usuario.
//!
//! Se leen y cambian con los cmdlets oficiales de Windows
//! (`Get/Set-WinUserLanguageList`, `Set-WinDefaultInputMethodOverride`) en
//! scripts fijos: lo único que se inserta en ellos son etiquetas de idioma y
//! códigos de teclado ya validados (letras, números, guiones y llaves).

use serde::{Deserialize, Serialize};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::win::from_wide;
use crate::util::{process, reg};

const LAYOUTS_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Keyboard Layouts";

/// Idiomas que se ofrecen para agregar (etiqueta, nombre).
pub const LANGUAGES: [(&str, &str); 14] = [
    ("es-AR", "Español (Argentina)"),
    ("es-ES", "Español (España)"),
    ("es-MX", "Español (México)"),
    ("es-CL", "Español (Chile)"),
    ("en-US", "Inglés (Estados Unidos)"),
    ("en-GB", "Inglés (Reino Unido)"),
    ("pt-BR", "Portugués (Brasil)"),
    ("fr-FR", "Francés"),
    ("de-DE", "Alemán"),
    ("it-IT", "Italiano"),
    ("ja-JP", "Japonés"),
    ("ko-KR", "Coreano"),
    ("zh-CN", "Chino (simplificado)"),
    ("ru-RU", "Ruso"),
];

/// Distribuciones que se ofrecen para agregar (código KLID, nombre).
pub const LAYOUTS: [(&str, &str); 10] = [
    ("0000080A", "Latinoamericano"),
    ("0000040A", "Español"),
    ("00000409", "Estados Unidos"),
    ("00020409", "Estados Unidos-Internacional"),
    ("00000809", "Reino Unido"),
    ("00000416", "Portugués (Brasil ABNT)"),
    ("00010416", "Portugués (Brasil ABNT2)"),
    ("0000040C", "Francés"),
    ("00000407", "Alemán"),
    ("00000410", "Italiano"),
];

#[derive(Debug, Clone, Serialize)]
pub struct KeyboardLayout {
    /// Código de Windows del método de entrada ("2C0A:0000080A").
    pub tip: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputLanguage {
    pub tag: String,
    pub name: String,
    pub layouts: Vec<KeyboardLayout>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Languages {
    /// En orden de preferencia (el primero es el idioma principal).
    pub list: Vec<InputLanguage>,
    /// Distribución elegida como predeterminada (si no hay, Windows usa la del primer idioma).
    pub default_tip: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawLanguage {
    tag: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tips: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawState {
    #[serde(default)]
    languages: Vec<RawLanguage>,
    #[serde(default)]
    default: Option<String>,
}

const LIST_SCRIPT: &str = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
# `Get-WinUserLanguageList` devuelve un solo objeto `WinUserLanguageList`
# (una colección tipada), no un idioma por vez: ni pipe (`|`) ni `@(...)`
# alrededor del cmdlet lo desarman en la pipeline, así que `$_.LanguageTag`
# sobre más de un idioma da un array que PowerShell junta con espacios al
# convertirlo a texto (dos idiomas quedaban pegados en uno solo, con sus
# distribuciones mezcladas). La sentencia `foreach` sí recorre cada elemento
# de verdad (llama a GetEnumerator()), a diferencia del cmdlet ForEach-Object.
$langs = @()
foreach ($lang in (Get-WinUserLanguageList)) {
    $langs += [pscustomobject]@{
        Tag = [string]$lang.LanguageTag
        Name = [string]$lang.LocalizedName
        Tips = @($lang.InputMethodTips | ForEach-Object { [string]$_ })
    }
}
# Mismo motivo acá: forzar la lista para que `.InputMethodTip` no se
# convierta en un array si hubiera más de un resultado.
$override = @(Get-WinDefaultInputMethodOverride)
$defaultTip = if ($override.Count -gt 0) { [string]$override[0].InputMethodTip } else { $null }
[pscustomobject]@{ Languages = $langs; Default = $defaultTip } | ConvertTo-Json -Depth 4 -Compress
"#;

/// Etiqueta de idioma BCP 47 simple: "es", "es-AR", "zh-Hans-CN".
fn valid_tag(tag: &str) -> AppResult<&str> {
    let mut parts = tag.split('-');
    let first_ok =
        parts.next().is_some_and(|p| (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic()));
    let rest_ok = parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()));
    if first_ok && rest_ok && tag.len() <= 35 { Ok(tag) } else { Err(AppError::invalid("Idioma inválido.")) }
}

/// Código de distribución de 8 dígitos hexadecimales ("0000080A").
fn valid_klid(klid: &str) -> AppResult<&str> {
    if klid.len() == 8 && klid.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(klid)
    } else {
        Err(AppError::invalid("Distribución de teclado inválida."))
    }
}

/// Método de entrada: "2C0A:0000080A" o, para editores de idioma, "0411:{GUID}{GUID}".
fn valid_tip(tip: &str) -> AppResult<&str> {
    let ok = tip.len() <= 90
        && tip.split_once(':').is_some_and(|(lang, rest)| {
            lang.len() == 4
                && lang.chars().all(|c| c.is_ascii_hexdigit())
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, '{' | '}' | '-'))
        });
    if ok { Ok(tip) } else { Err(AppError::invalid("Método de entrada inválido.")) }
}

/// Nombre de una distribución en el idioma de Windows ("Latinoamericano").
fn layout_name(tip: &str, language: &str) -> String {
    let Some((_, klid)) = tip.split_once(':') else { return tip.to_owned() };
    if klid.starts_with('{') {
        return format!("{language} (editor de entrada)");
    }
    let key = format!(r"{LAYOUTS_KEY}\{klid}");
    reg::read_string(&key, "Layout Display Name")
        .and_then(|indirect| {
            let mut buffer = [0u16; 260];
            unsafe { SHLoadIndirectString(&HSTRING::from(indirect), &mut buffer, None) }.ok()?;
            Some(from_wide(&buffer))
        })
        .filter(|name| !name.is_empty())
        .or_else(|| reg::read_string(&key, "Layout Text"))
        .unwrap_or_else(|| klid.to_owned())
}

pub fn list() -> AppResult<Languages> {
    let output = process::powershell("leer idiomas", LIST_SCRIPT)?;
    let raw: RawState = serde_json::from_str(output.trim())
        .map_err(|err| AppError::failed(format!("No se pudo leer la lista de idiomas: {err}")))?;
    let list = raw
        .languages
        .into_iter()
        .map(|language| InputLanguage {
            layouts: language
                .tips
                .iter()
                .map(|tip| KeyboardLayout { name: layout_name(tip, &language.name), tip: tip.clone() })
                .collect(),
            name: if language.name.is_empty() { language.tag.clone() } else { language.name },
            tag: language.tag,
        })
        .collect();
    Ok(Languages { list, default_tip: raw.default.filter(|tip| !tip.is_empty()) })
}

fn run(label: &str, script: &str) -> AppResult<()> {
    process::powershell(label, script).map(|_| ())
}

pub fn add_language(tag: &str) -> AppResult<()> {
    let tag = valid_tag(tag)?;
    run(
        "agregar idioma",
        &format!(
            "$l = Get-WinUserLanguageList; $l.Add('{tag}'); Set-WinUserLanguageList $l -Force -WarningAction SilentlyContinue"
        ),
    )?;
    log::info!(target: "teclado", "Se agregó el idioma {tag}");
    Ok(())
}

pub fn remove_language(tag: &str) -> AppResult<()> {
    let tag = valid_tag(tag)?;
    let current = list()?;
    if current.list.len() <= 1 {
        return Err(AppError::invalid("Tiene que quedar al menos un idioma."));
    }
    run(
        "quitar idioma",
        &format!(
            "$l = Get-WinUserLanguageList; $x = $l | Where-Object {{ $_.LanguageTag -eq '{tag}' }}; \
             if ($x) {{ [void]$l.Remove($x); Set-WinUserLanguageList $l -Force -WarningAction SilentlyContinue }}"
        ),
    )?;
    log::info!(target: "teclado", "Se quitó el idioma {tag}");
    Ok(())
}

/// Agrega una distribución de teclado a un idioma (p. ej. "Estados Unidos-Internacional" a Español).
pub fn add_layout(tag: &str, klid: &str) -> AppResult<()> {
    let tag = valid_tag(tag)?;
    let klid = valid_klid(klid)?;
    run(
        "agregar teclado",
        &format!(
            "$l = Get-WinUserLanguageList; $x = $l | Where-Object {{ $_.LanguageTag -eq '{tag}' }}; \
             $id = [Globalization.CultureInfo]::GetCultureInfo('{tag}').KeyboardLayoutId -band 0xFFFF; \
             $tip = ('{{0:X4}}:{klid}' -f $id); \
             if ($x -and -not $x.InputMethodTips.Contains($tip)) {{ $x.InputMethodTips.Add($tip); \
             Set-WinUserLanguageList $l -Force -WarningAction SilentlyContinue }}"
        ),
    )?;
    log::info!(target: "teclado", "Se agregó el teclado {klid} a {tag}");
    Ok(())
}

pub fn remove_layout(tag: &str, tip: &str) -> AppResult<()> {
    let tag = valid_tag(tag)?;
    let tip = valid_tip(tip)?;
    let current = list()?;
    let language = current.list.iter().find(|language| language.tag == tag);
    if language.is_none_or(|language| language.layouts.len() <= 1) {
        return Err(AppError::invalid("Cada idioma necesita al menos un teclado."));
    }
    run(
        "quitar teclado",
        &format!(
            "$l = Get-WinUserLanguageList; $x = $l | Where-Object {{ $_.LanguageTag -eq '{tag}' }}; \
             if ($x) {{ [void]$x.InputMethodTips.Remove('{tip}'); Set-WinUserLanguageList $l -Force -WarningAction SilentlyContinue }}"
        ),
    )?;
    log::info!(target: "teclado", "Se quitó el teclado {tip} de {tag}");
    Ok(())
}

/// Distribución que se usa al iniciar sesión y en las apps nuevas.
pub fn set_default_layout(tip: &str) -> AppResult<()> {
    let tip = valid_tip(tip)?;
    run("teclado predeterminado", &format!("Set-WinDefaultInputMethodOverride -InputTip '{tip}'"))?;
    log::info!(target: "teclado", "Teclado predeterminado: {tip}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LANGUAGES, LAYOUTS, valid_klid, valid_tag, valid_tip};

    #[test]
    fn only_safe_codes_reach_powershell() {
        assert!(valid_tag("es-AR").is_ok());
        assert!(valid_tag("zh-Hans-CN").is_ok());
        assert!(valid_tag("es'; rm").is_err());
        assert!(valid_tag("").is_err());
        assert!(valid_klid("0000080A").is_ok());
        assert!(valid_klid("0000080A'").is_err());
        assert!(valid_tip("2C0A:0000080A").is_ok());
        assert!(valid_tip("0411:{03B5835F-F03C-411B-9CE2-AA23E1171E36}{A76C93D9-5523-4E90-AAFA-4DB112F9AC76}").is_ok());
        assert!(valid_tip("2C0A:0000080A'; x").is_err());
    }

    #[test]
    fn offered_codes_are_valid() {
        assert!(LANGUAGES.iter().all(|(tag, _)| valid_tag(tag).is_ok()));
        assert!(LAYOUTS.iter().all(|(klid, _)| valid_klid(klid).is_ok()));
    }
}

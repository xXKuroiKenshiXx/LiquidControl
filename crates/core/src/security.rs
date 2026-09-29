//! Seguridad y antivirus: estado y ajustes de Windows Defender (protección en
//! tiempo real, protección en la nube) y del Firewall de Windows (los tres
//! perfiles), análisis y amenazas detectadas.
//!
//! No hay una API Win32 pública y estable para todo esto: se usan los mismos
//! cmdlets de PowerShell que trae Windows (módulo `Defender`), en scripts fijos
//! (`process::powershell`) que solo reciben `true`/`false` o un nombre de perfil
//! ya validado. Cambiar cualquiera de estos ajustes requiere administrador.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::util::process;
use crate::util::win::require_admin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirewallProfile {
    Domain,
    Private,
    Public,
}

impl FirewallProfile {
    pub const ALL: [FirewallProfile; 3] = [Self::Domain, Self::Private, Self::Public];

    fn ps_name(self) -> &'static str {
        match self {
            Self::Domain => "Domain",
            Self::Private => "Private",
            Self::Public => "Public",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Domain => "Red de dominio",
            Self::Private => "Red privada",
            Self::Public => "Red pública",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallStatus {
    pub profile: FirewallProfile,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityStatus {
    /// `None` si Defender no está instalado o no responde (p. ej. hay otro antivirus).
    pub defender_installed: bool,
    pub antivirus_enabled: bool,
    pub realtime_protection: bool,
    pub cloud_protection: bool,
    pub tamper_protection: bool,
    /// "AAAA-MM-DD HH:mm" o vacío si nunca se analizó.
    pub last_scan: String,
    pub signature_age_days: u32,
    pub threats_found: u32,
    pub firewalls: Vec<FirewallStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Threat {
    pub id: String,
    pub name: String,
    pub path: String,
    /// 1 = bajo, 2 = moderado, 3 = alto, 4 = severo, 5 = crítico (escala de Defender).
    pub severity: u32,
    /// "AAAA-MM-DD HH:mm".
    pub detected: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
    Quick,
    Full,
}

impl ScanKind {
    fn ps_value(self) -> &'static str {
        match self {
            Self::Quick => "QuickScan",
            Self::Full => "FullScan",
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct RawStatus {
    #[serde(default)]
    antivirus_enabled: Option<bool>,
    #[serde(default)]
    real_time_protection_enabled: bool,
    #[serde(default)]
    cloud_protection: bool,
    #[serde(default)]
    is_tamper_protected: bool,
    #[serde(default)]
    quick_scan_end_time: String,
    #[serde(default)]
    full_scan_end_time: String,
    #[serde(default)]
    antivirus_signature_age: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct RawFirewall {
    name: String,
    enabled: bool,
}

// El objeto que devuelve `Get-MpComputerStatus` es enorme (con la definición
// entera de su clase CIM); serializarlo tal cual infla la memoria del proceso
// y tarda de más al parsear. El script arma a mano un objeto chico y plano con
// solo los campos que hacen falta. Las fechas se formatean del lado de
// PowerShell: pueden llegar como `[DateTime]` real o, en algunas versiones,
// como el "/Date(...)/ " de JSON.NET, así que nunca se parsean en Rust.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct RawState {
    #[serde(default)]
    defender: Option<RawStatus>,
    #[serde(default)]
    firewalls: Vec<RawFirewall>,
    #[serde(default)]
    threats: u32,
}

const STATUS_SCRIPT: &str = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
function Format-Date($value) {
    if ($value -and $value -is [DateTime] -and $value.Year -gt 1601) { return $value.ToString('yyyy-MM-dd HH:mm') }
    return ''
}
$raw = $null
try { $raw = Get-MpComputerStatus -ErrorAction Stop } catch {}
$defender = $null
if ($raw) {
    $cloud = $false
    if ($raw.PSObject.Properties['MAPSReporting']) { $cloud = [int]$raw.MAPSReporting -ne 0 }
    elseif ($raw.PSObject.Properties['MAPSReportingEnabled']) { $cloud = [bool]$raw.MAPSReportingEnabled }
    $defender = [ordered]@{
        AntivirusEnabled = [bool]$raw.AntivirusEnabled
        RealTimeProtectionEnabled = [bool]$raw.RealTimeProtectionEnabled
        CloudProtection = $cloud
        IsTamperProtected = [bool]$raw.IsTamperProtected
        AntivirusSignatureAge = [int]$raw.AntivirusSignatureAge
        QuickScanEndTime = Format-Date $raw.QuickScanEndTime
        FullScanEndTime = Format-Date $raw.FullScanEndTime
    }
}
$fw = @(Get-NetFirewallProfile -ErrorAction SilentlyContinue | ForEach-Object {
    [pscustomobject]@{ Name = $_.Name; Enabled = [bool]$_.Enabled }
})
$threats = 0
try { $threats = @(Get-MpThreatDetection -ErrorAction Stop).Count } catch {}
[pscustomobject]@{ Defender = $defender; Firewalls = $fw; Threats = $threats } | ConvertTo-Json -Depth 3 -Compress
"#;

const THREATS_SCRIPT: &str = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$list = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | ForEach-Object {
    $threat = Get-MpThreat -ThreatID $_.ThreatID -ErrorAction SilentlyContinue
    $detected = ''
    if ($_.InitialDetectionTime -and $_.InitialDetectionTime.Year -gt 1601) {
        $detected = $_.InitialDetectionTime.ToString('yyyy-MM-dd HH:mm')
    }
    [pscustomobject]@{
        Id = "$($_.ThreatID)"
        Name = $(if ($threat) { $threat.ThreatName } else { "Amenaza" })
        Path = $(if ($threat -and $threat.Resources) { ($threat.Resources -join '; ') } else { '' })
        Severity = $(if ($threat) { [int]$threat.SeverityID } else { 0 })
        Detected = $detected
    }
})
$list | ConvertTo-Json -Depth 2 -Compress
"#;

/// Solo dígitos, letras, guiones y llaves: id de amenaza válido de Defender.
fn valid_threat_id(id: &str) -> AppResult<&str> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '{' | '}'));
    if ok { Ok(id) } else { Err(AppError::invalid("Identificador de amenaza inválido.")) }
}

pub fn status() -> AppResult<SecurityStatus> {
    let output = process::powershell("estado de seguridad", STATUS_SCRIPT)?;
    let raw: RawState = serde_json::from_str(output.trim())
        .map_err(|err| AppError::failed(format!("No se pudo leer el estado de seguridad: {err}")))?;
    let firewalls = FirewallProfile::ALL
        .iter()
        .map(|profile| FirewallStatus {
            profile: *profile,
            enabled: raw
                .firewalls
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(profile.ps_name()))
                .map(|entry| entry.enabled)
                .unwrap_or(true),
        })
        .collect();
    let defender = raw.defender.unwrap_or_default();
    let last_scan =
        if defender.full_scan_end_time.is_empty() { defender.quick_scan_end_time } else { defender.full_scan_end_time };
    Ok(SecurityStatus {
        defender_installed: defender.antivirus_enabled.is_some(),
        antivirus_enabled: defender.antivirus_enabled.unwrap_or(false),
        realtime_protection: defender.real_time_protection_enabled,
        cloud_protection: defender.cloud_protection,
        tamper_protection: defender.is_tamper_protected,
        last_scan,
        signature_age_days: defender.antivirus_signature_age,
        threats_found: raw.threats,
        firewalls,
    })
}

pub fn threats() -> AppResult<Vec<Threat>> {
    let output = process::powershell("amenazas detectadas", THREATS_SCRIPT)?;
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    // Un solo resultado llega como objeto, no como arreglo.
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|err| AppError::failed(format!("No se pudo leer las amenazas: {err}")))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Null => Vec::new(),
        single => vec![single],
    };
    items
        .into_iter()
        .map(|item| serde_json::from_value(item).map_err(AppError::from))
        .collect::<AppResult<Vec<Threat>>>()
}

fn run(label: &str, script: &str) -> AppResult<()> {
    require_admin()?;
    process::powershell(label, script).map(|_| ())
}

pub fn set_realtime_protection(on: bool) -> AppResult<()> {
    let flag = if on { "$false" } else { "$true" };
    run("protección en tiempo real", &format!("Set-MpPreference -DisableRealtimeMonitoring {flag}"))?;
    log::info!(target: "seguridad", "Protección en tiempo real: {}", if on { "activada" } else { "desactivada" });
    Ok(())
}

pub fn set_cloud_protection(on: bool) -> AppResult<()> {
    let level = if on { "Advanced" } else { "Disabled" };
    run(
        "protección en la nube",
        &format!(
            "Set-MpPreference -MAPSReporting {level} -SubmitSamplesConsent {}",
            if on { "SendSafeSamples" } else { "NeverSend" }
        ),
    )?;
    log::info!(target: "seguridad", "Protección en la nube: {}", if on { "activada" } else { "desactivada" });
    Ok(())
}

pub fn set_firewall(profile: FirewallProfile, on: bool) -> AppResult<()> {
    let value = if on { "True" } else { "False" };
    run("firewall", &format!("Set-NetFirewallProfile -Profile {} -Enabled {value}", profile.ps_name()))?;
    log::info!(target: "seguridad", "Firewall ({}): {}", profile.label(), if on { "activado" } else { "desactivado" });
    Ok(())
}

/// Inicia un análisis. No espera a que termine (puede tardar minutos u horas):
/// Windows lo sigue en segundo plano y su resultado se ve con `status()`.
pub fn start_scan(kind: ScanKind) -> AppResult<()> {
    require_admin()?;
    let script = format!(
        "Start-Process -FilePath powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-MpScan -ScanType {}'",
        kind.ps_value()
    );
    process::powershell("iniciar análisis", &script)?;
    log::info!(target: "seguridad", "Se inició un análisis ({})", kind.ps_value());
    Ok(())
}

/// Pone en cuarentena (o quita) la amenaza indicada.
pub fn remove_threat(id: &str) -> AppResult<()> {
    let id = valid_threat_id(id)?;
    run("quitar amenaza", &format!("Remove-MpThreat -ThreatID '{id}'"))?;
    log::info!(target: "seguridad", "Se quitó la amenaza {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::valid_threat_id;

    #[test]
    fn only_safe_ids_reach_powershell() {
        assert!(valid_threat_id("123456").is_ok());
        assert!(valid_threat_id("{03B5835F-F03C-411B-9CE2-AA23E1171E36}").is_ok());
        assert!(valid_threat_id("123'; rm -rf").is_err());
        assert!(valid_threat_id("").is_err());
    }
}

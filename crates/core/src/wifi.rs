//! Wi‑Fi con la API nativa WLAN (estado, escaneo, conexión, perfiles) y la API
//! WinRT de radios para encender/apagar. Las contraseñas nunca se registran en logs.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_AUTH_ALGO_80211_OPEN, DOT11_AUTH_ALGO_80211_SHARED_KEY, DOT11_AUTH_ALGO_OWE, DOT11_AUTH_ALGO_RSNA,
    DOT11_AUTH_ALGO_RSNA_PSK, DOT11_AUTH_ALGO_WPA, DOT11_AUTH_ALGO_WPA_PSK, DOT11_AUTH_ALGO_WPA3_ENT,
    DOT11_AUTH_ALGO_WPA3_ENT_192, DOT11_AUTH_ALGO_WPA3_SAE, DOT11_AUTH_ALGORITHM, DOT11_CIPHER_ALGO_TKIP,
    DOT11_CIPHER_ALGORITHM, DOT11_SSID, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_ATTRIBUTES,
    WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanConnect, WlanDeleteProfile,
    WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList, WlanOpenHandle,
    WlanQueryInterface, WlanScan, WlanSetInterface, WlanSetProfile, dot11_BSS_type_any, wlan_connection_mode_profile,
    wlan_interface_state_connected, wlan_intf_opcode_autoconf_enabled, wlan_intf_opcode_current_connection,
};
use windows::core::{GUID, HSTRING, PCWSTR};

use crate::error::{AppError, AppResult};
use crate::util::com;
use crate::util::win::from_wide;

const ERROR_ACCESS_DENIED: u32 = 5;
/// Lo devuelve la API cuando WLAN AutoConfig está desactivado en la interfaz.
const ERROR_NOT_SUPPORTED: u32 = 50;
const NETWORK_CONNECTED: u32 = 0x1;
const NETWORK_HAS_PROFILE: u32 = 0x2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

fn location_denied() -> AppError {
    AppError::Coded("location_denied", "Windows bloquea el acceso a las redes Wi‑Fi sin permiso de ubicación.".into())
}

fn autoconfig_off() -> AppError {
    AppError::Coded("autoconfig_off", "La configuración automática de Wi‑Fi está desactivada en este adaptador.".into())
}

fn check(code: u32) -> AppResult<()> {
    match code {
        0 => Ok(()),
        ERROR_ACCESS_DENIED => Err(location_denied()),
        ERROR_NOT_SUPPORTED => Err(autoconfig_off()),
        other => Err(windows::core::Error::from(windows::core::HRESULT::from_win32(other)).into()),
    }
}

/// Sesión WLAN que se cierra sola al salir de alcance.
struct Client(HANDLE);

impl Client {
    fn open() -> AppResult<Self> {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        check(unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) })?;
        Ok(Self(handle))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            let _ = WlanCloseHandle(self.0, None);
        }
    }
}

fn ssid_string(ssid: &DOT11_SSID) -> String {
    let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
    String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned()
}

fn auth_name(algo: DOT11_AUTH_ALGORITHM, secured: bool) -> &'static str {
    match algo {
        DOT11_AUTH_ALGO_80211_OPEN if !secured => "Abierta",
        DOT11_AUTH_ALGO_80211_OPEN | DOT11_AUTH_ALGO_80211_SHARED_KEY => "WEP",
        DOT11_AUTH_ALGO_WPA_PSK => "WPA-Personal",
        DOT11_AUTH_ALGO_RSNA_PSK => "WPA2-Personal",
        DOT11_AUTH_ALGO_WPA3_SAE => "WPA3-Personal",
        DOT11_AUTH_ALGO_OWE => "Abierta mejorada (OWE)",
        DOT11_AUTH_ALGO_WPA | DOT11_AUTH_ALGO_RSNA | DOT11_AUTH_ALGO_WPA3_ENT | DOT11_AUTH_ALGO_WPA3_ENT_192 => {
            "Empresarial"
        }
        _ => "Protegida",
    }
}

fn first_interface(client: &Client) -> AppResult<Option<(GUID, String)>> {
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    check(unsafe { WlanEnumInterfaces(client.0, None, &mut list) })?;
    let result = unsafe {
        let info_list = &*list;
        (info_list.dwNumberOfItems > 0).then(|| {
            let info = &*info_list.InterfaceInfo.as_ptr();
            (info.InterfaceGuid, from_wide(&info.strInterfaceDescription))
        })
    };
    unsafe { WlanFreeMemory(list as *const c_void) };
    Ok(result)
}

/// ¿Está activa la configuración automática (el servicio que busca y conecta redes)?
fn autoconfig_enabled(client: &Client, guid: &GUID) -> bool {
    let mut size = 0u32;
    let mut data: *mut c_void = std::ptr::null_mut();
    let code = unsafe {
        WlanQueryInterface(client.0, guid, wlan_intf_opcode_autoconf_enabled, None, &mut size, &mut data, None)
    };
    if code != 0 || data.is_null() {
        return true;
    }
    let enabled = unsafe { *(data as *const i32) } != 0;
    unsafe { WlanFreeMemory(data) };
    enabled
}

/// Atributos de la conexión actual; `Err(location_denied)` si Windows no deja leerla.
fn current_connection(client: &Client, guid: &GUID) -> AppResult<Option<WLAN_CONNECTION_ATTRIBUTES>> {
    let mut size = 0u32;
    let mut data: *mut c_void = std::ptr::null_mut();
    let code = unsafe {
        WlanQueryInterface(client.0, guid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None)
    };
    if code == ERROR_ACCESS_DENIED {
        return Err(location_denied());
    }
    if code != 0 || data.is_null() {
        // ERROR_INVALID_STATE: no está conectado.
        return Ok(None);
    }
    let attributes = unsafe { *(data as *const WLAN_CONNECTION_ATTRIBUTES) };
    unsafe { WlanFreeMemory(data) };
    Ok(Some(attributes))
}

#[derive(Debug, Clone, Serialize)]
pub struct WifiNetwork {
    pub ssid: String,
    /// Calidad de señal 0–100.
    pub signal: u32,
    pub secured: bool,
    pub auth: String,
    pub connected: bool,
    pub profile: Option<String>,
}

struct Scanned {
    view: WifiNetwork,
    algo: DOT11_AUTH_ALGORITHM,
    cipher: DOT11_CIPHER_ALGORITHM,
}

fn available(client: &Client, guid: &GUID) -> AppResult<Vec<Scanned>> {
    let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
    check(unsafe { WlanGetAvailableNetworkList(client.0, guid, 0, None, &mut list) })?;
    // Una misma red puede aparecer varias veces (con y sin perfil): se queda la mejor entrada.
    let mut by_ssid: HashMap<String, Scanned> = HashMap::new();
    unsafe {
        let networks = &*list;
        for index in 0..networks.dwNumberOfItems as usize {
            let network = &*networks.Network.as_ptr().add(index);
            let ssid = ssid_string(&network.dot11Ssid);
            if ssid.is_empty() {
                continue;
            }
            let secured = network.bSecurityEnabled.as_bool();
            let profile = from_wide(&network.strProfileName);
            let candidate = Scanned {
                view: WifiNetwork {
                    ssid: ssid.clone(),
                    signal: network.wlanSignalQuality,
                    secured,
                    auth: auth_name(network.dot11DefaultAuthAlgorithm, secured).into(),
                    connected: network.dwFlags & NETWORK_CONNECTED != 0,
                    profile: (network.dwFlags & NETWORK_HAS_PROFILE != 0 && !profile.is_empty()).then_some(profile),
                },
                algo: network.dot11DefaultAuthAlgorithm,
                cipher: network.dot11DefaultCipherAlgorithm,
            };
            let score = |s: &Scanned| (s.view.connected, s.view.profile.is_some(), s.view.signal);
            match by_ssid.get(&ssid) {
                Some(existing) if score(existing) >= score(&candidate) => {}
                _ => {
                    by_ssid.insert(ssid, candidate);
                }
            }
        }
        WlanFreeMemory(list as *const c_void);
    }
    let mut result: Vec<Scanned> = by_ssid.into_values().collect();
    result.sort_by(|a, b| b.view.connected.cmp(&a.view.connected).then(b.view.signal.cmp(&a.view.signal)));
    Ok(result)
}

fn wifi_radio() -> AppResult<Option<Radio>> {
    let radios = Radio::GetRadiosAsync()?.get()?;
    for index in 0..radios.Size()? {
        let radio = radios.GetAt(index)?;
        if radio.Kind()? == RadioKind::WiFi {
            return Ok(Some(radio));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Serialize)]
pub struct WifiConnection {
    pub ssid: String,
    pub signal: u32,
    pub secured: bool,
    pub auth: String,
    pub rx_mbps: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WifiStatus {
    /// Descripción del adaptador, o `None` si la PC no tiene Wi‑Fi.
    pub adapter: Option<String>,
    pub radio_on: bool,
    /// WLAN AutoConfig activo (si no, Windows no busca redes).
    pub auto_config: bool,
    pub location_denied: bool,
    pub connection: Option<WifiConnection>,
}

pub fn status() -> AppResult<WifiStatus> {
    com::mta(|| {
        let empty =
            WifiStatus { adapter: None, radio_on: false, auto_config: true, location_denied: false, connection: None };
        // Sin servicio WLAN (PC sin Wi‑Fi) WlanOpenHandle falla: se informa "sin adaptador".
        let Ok(client) = Client::open() else { return Ok(empty) };
        let Some((guid, adapter)) = first_interface(&client)? else { return Ok(empty) };
        let auto_config = autoconfig_enabled(&client, &guid);
        let radio_on = wifi_radio().ok().flatten().is_none_or(|radio| radio.State().ok() == Some(RadioState::On));
        let (connection, location_denied) = match current_connection(&client, &guid) {
            Ok(attrs) => (attrs, false),
            Err(AppError::Coded("location_denied", _)) => (None, true),
            Err(err) => return Err(err),
        };
        let connection = connection.filter(|a| a.isState == wlan_interface_state_connected).map(|a| {
            let assoc = a.wlanAssociationAttributes;
            let security = a.wlanSecurityAttributes;
            let secured = security.bSecurityEnabled.as_bool();
            WifiConnection {
                ssid: ssid_string(&assoc.dot11Ssid),
                signal: assoc.wlanSignalQuality,
                secured,
                auth: auth_name(security.dot11AuthAlgorithm, secured).into(),
                rx_mbps: (assoc.ulRxRate > 0).then_some(assoc.ulRxRate / 1000),
            }
        });
        Ok(WifiStatus { adapter: Some(adapter), radio_on, auto_config, location_denied, connection })
    })
}

/// Busca redes cercanas (tarda ~1,5 s: le da tiempo al adaptador para escanear).
pub fn scan() -> AppResult<Vec<WifiNetwork>> {
    com::mta(|| {
        let client = Client::open()?;
        let Some((guid, _)) = first_interface(&client)? else { return Ok(Vec::new()) };
        if !autoconfig_enabled(&client, &guid) {
            return Err(autoconfig_off());
        }
        if unsafe { WlanScan(client.0, &guid, None, None, None) } == 0 {
            std::thread::sleep(Duration::from_millis(1500));
        }
        Ok(available(&client, &guid)?.into_iter().map(|n| n.view).collect())
    })
}

fn xml_escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            other => other.to_string(),
        })
        .collect()
}

/// Perfil WLAN (XML) para una red personal o abierta.
fn profile_xml(network: &Scanned, password: Option<&str>) -> AppResult<String> {
    let (auth, cipher) = match network.algo {
        DOT11_AUTH_ALGO_80211_OPEN if !network.view.secured => ("open", "none"),
        DOT11_AUTH_ALGO_OWE => ("OWE", "AES"),
        DOT11_AUTH_ALGO_RSNA_PSK => ("WPA2PSK", "AES"),
        DOT11_AUTH_ALGO_WPA3_SAE => ("WPA3SAE", "AES"),
        DOT11_AUTH_ALGO_WPA_PSK => ("WPAPSK", if network.cipher == DOT11_CIPHER_ALGO_TKIP { "TKIP" } else { "AES" }),
        _ => {
            return Err(AppError::Coded(
                "unsupported",
                "Esta red usa un tipo de seguridad que hay que configurar desde Windows.".into(),
            ));
        }
    };
    let name = xml_escape(&network.view.ssid);
    let hex: String = network.view.ssid.bytes().map(|b| format!("{b:02X}")).collect();
    let key = password
        .map(|p| {
            format!(
                "<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
                xml_escape(p)
            )
        })
        .unwrap_or_default();
    Ok(format!(
        r#"<?xml version="1.0"?><WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1"><name>{name}</name><SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{auth}</authentication><encryption>{cipher}</encryption><useOneX>false</useOneX></authEncryption>{key}</security></MSM></WLANProfile>"#
    ))
}

fn connect_blocking(ssid: &str, password: Option<&str>) -> AppResult<()> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err(AppError::invalid("Nombre de red inválido."));
    }
    if password.is_some_and(|p| !(8..=63).contains(&p.chars().count())) {
        return Err(AppError::Coded("auth_failed", "La contraseña debe tener entre 8 y 63 caracteres.".into()));
    }
    let client = Client::open()?;
    let (guid, _) = first_interface(&client)?.ok_or_else(|| AppError::not_found("No hay un adaptador Wi‑Fi."))?;
    let networks = available(&client, &guid)?;
    let network = networks
        .iter()
        .find(|n| n.view.ssid == ssid)
        .ok_or_else(|| AppError::not_found("La red ya no está disponible."))?;

    let (profile, created) = match (&network.view.profile, password) {
        (Some(profile), None) => (profile.clone(), false),
        _ => {
            if network.view.secured && password.is_none() {
                return Err(AppError::Coded("needs_password", "Esta red requiere contraseña.".into()));
            }
            let xml = HSTRING::from(profile_xml(network, password)?);
            let mut reason = 0u32;
            check(unsafe { WlanSetProfile(client.0, &guid, 0, &xml, PCWSTR::null(), true, None, &mut reason) })?;
            (ssid.to_owned(), true)
        }
    };

    let profile_name = HSTRING::from(profile.as_str());
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(profile_name.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_any,
        dwFlags: 0,
    };
    check(unsafe { WlanConnect(client.0, &guid, &params, None) })?;

    // WlanConnect es asíncrono: se espera a que la conexión quede establecida.
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(Some(attrs)) = current_connection(&client, &guid) {
            if attrs.isState == wlan_interface_state_connected
                && ssid_string(&attrs.wlanAssociationAttributes.dot11Ssid) == ssid
            {
                return Ok(());
            }
        }
    }
    // No se deja guardado un perfil con una contraseña que no funcionó.
    if created {
        unsafe {
            let _ = WlanDeleteProfile(client.0, &guid, &profile_name, None);
        }
    }
    Err(AppError::Coded("auth_failed", "No se pudo conectar. Revisá la contraseña e intentá de nuevo.".into()))
}

/// Conecta a una red (con perfil guardado, abierta o con la contraseña indicada).
pub fn connect(ssid: &str, password: Option<&str>) -> AppResult<()> {
    // La contraseña nunca se registra.
    log::info!(target: "wifi", "Conectando a «{ssid}»");
    let ssid = ssid.to_owned();
    let password = password.map(str::to_owned);
    com::mta(move || connect_blocking(&ssid, password.as_deref()))
}

pub fn disconnect() -> AppResult<()> {
    log::info!(target: "wifi", "Desconectando");
    com::mta(|| {
        let client = Client::open()?;
        let (guid, _) = first_interface(&client)?.ok_or_else(|| AppError::not_found("No hay un adaptador Wi‑Fi."))?;
        check(unsafe { WlanDisconnect(client.0, &guid, None) })
    })
}

/// Olvida una red guardada.
pub fn forget(profile: &str) -> AppResult<()> {
    log::info!(target: "wifi", "Olvidando la red «{profile}»");
    let profile = HSTRING::from(profile);
    com::mta(move || {
        let client = Client::open()?;
        let (guid, _) = first_interface(&client)?.ok_or_else(|| AppError::not_found("No hay un adaptador Wi‑Fi."))?;
        check(unsafe { WlanDeleteProfile(client.0, &guid, &profile, None) })
    })
}

/// Activa o desactiva WLAN AutoConfig, el servicio que hace que Windows busque
/// y se conecte solo a redes Wi‑Fi (equivale a `netsh wlan set autoconfig`).
/// Desactivado, esta pantalla no puede escanear ni conectar (Windows deja de
/// vigilar la interfaz), pero la radio se puede seguir prendiendo y apagando.
pub fn set_autoconfig(enabled: bool) -> AppResult<()> {
    log::info!(target: "wifi", "WLAN AutoConfig: {}", if enabled { "activado" } else { "desactivado" });
    com::mta(move || {
        let client = Client::open()?;
        let (guid, _) = first_interface(&client)?.ok_or_else(|| AppError::not_found("No hay un adaptador Wi‑Fi."))?;
        let value: i32 = enabled.into();
        let code = unsafe {
            WlanSetInterface(
                client.0,
                &guid,
                wlan_intf_opcode_autoconf_enabled,
                size_of::<i32>() as u32,
                &value as *const i32 as *const c_void,
                None,
            )
        };
        match code {
            ERROR_ACCESS_DENIED => Err(AppError::NeedsAdmin),
            other => check(other),
        }
    })
}

/// Enciende o apaga la radio Wi‑Fi.
pub fn set_radio(on: bool) -> AppResult<()> {
    log::info!(target: "wifi", "Radio Wi‑Fi {}", if on { "encendida" } else { "apagada" });
    com::mta(move || {
        if Radio::RequestAccessAsync()?.get()? != RadioAccessStatus::Allowed {
            return Err(AppError::failed("Windows no permite controlar la radio Wi‑Fi."));
        }
        let radio = wifi_radio()?.ok_or_else(|| AppError::not_found("No hay una radio Wi‑Fi."))?;
        let status = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.get()?;
        if status == RadioAccessStatus::Allowed {
            Ok(())
        } else {
            Err(AppError::failed("No se pudo cambiar el Wi‑Fi."))
        }
    })
}

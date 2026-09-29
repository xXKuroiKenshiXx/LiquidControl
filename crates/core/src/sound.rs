//! Sonido: volumen y silencio (Core Audio), dispositivos y sonidos del sistema.

use serde::Serialize;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, EDataFlow, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications,
    eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::{HSTRING, PCWSTR};

use self::policy::{IPolicyConfig, POLICY_CONFIG_CLIENT, set_default_endpoint};
use crate::error::{AppError, AppResult};
use crate::util::win::require_admin;
use crate::util::{com, reg};

const BOOT_ANIMATION: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Authentication\LogonUI\BootAnimation";
const SCHEMES: &str = r"HKCU\AppEvents\Schemes";

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SoundStatus {
    /// 0–100.
    pub volume: f32,
    pub muted: bool,
    pub outputs: Vec<AudioDevice>,
    pub inputs: Vec<AudioDevice>,
    pub startup_sound: bool,
    pub system_sounds: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFlag {
    StartupSound,
    SystemSounds,
}

/// Interfaz de Windows para elegir el dispositivo de audio predeterminado. No está
/// en el SDK público, pero es la que usa el propio panel de Sonido desde Windows 7
/// (y herramientas como SoundSwitch). Solo se usa `SetDefaultEndpoint`; los demás
/// métodos existen para respetar el orden de la tabla virtual. Los nombres siguen
/// los de Windows, por eso el módulo permite nombres que no son snake_case.
#[allow(non_snake_case)]
mod policy {
    use windows::Win32::Media::Audio::ERole;
    use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        unsafe fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
        unsafe fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut core::ffi::c_void) -> HRESULT;
        unsafe fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        unsafe fn SetDeviceFormat(
            &self,
            device: PCWSTR,
            endpoint: *mut core::ffi::c_void,
            mix: *mut core::ffi::c_void,
        ) -> HRESULT;
        unsafe fn GetProcessingPeriod(
            &self,
            device: PCWSTR,
            default: i32,
            period: *mut i64,
            minimum: *mut i64,
        ) -> HRESULT;
        unsafe fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        unsafe fn GetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
        unsafe fn SetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
        unsafe fn GetPropertyValue(
            &self,
            device: PCWSTR,
            store: i32,
            key: *const core::ffi::c_void,
            value: *mut core::ffi::c_void,
        ) -> HRESULT;
        unsafe fn SetPropertyValue(
            &self,
            device: PCWSTR,
            store: i32,
            key: *const core::ffi::c_void,
            value: *mut core::ffi::c_void,
        ) -> HRESULT;
        unsafe fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        unsafe fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    pub const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Los métodos que genera `#[interface]` son privados a este módulo.
    pub fn set_default_endpoint(policy: &IPolicyConfig, device: PCWSTR, role: ERole) -> windows::core::Result<()> {
        unsafe { policy.SetDefaultEndpoint(device, role).ok() }
    }
}

fn enumerator() -> AppResult<IMMDeviceEnumerator> {
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
}

fn device_id(device: &IMMDevice) -> Option<String> {
    unsafe {
        let raw = device.GetId().ok()?;
        let id = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        id
    }
}

fn device_name(device: &IMMDevice) -> String {
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .map(|value| value.to_string())
            .unwrap_or_default()
    }
}

fn devices(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> AppResult<Vec<AudioDevice>> {
    unsafe {
        let default_id = enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| device_id(&d));
        let collection = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let mut list = Vec::new();
        for index in 0..collection.GetCount()? {
            let Ok(device) = collection.Item(index) else { continue };
            let id = device_id(&device).unwrap_or_default();
            list.push(AudioDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                name: device_name(&device),
                id,
            });
        }
        Ok(list)
    }
}

fn endpoint_volume(enumerator: &IMMDeviceEnumerator) -> AppResult<IAudioEndpointVolume> {
    unsafe {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        Ok(device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?)
    }
}

pub fn status() -> AppResult<SoundStatus> {
    com::mta(|| {
        let enumerator = enumerator()?;
        let (volume, muted) = match endpoint_volume(&enumerator) {
            Ok(endpoint) => unsafe {
                (
                    endpoint.GetMasterVolumeLevelScalar().unwrap_or(0.0) * 100.0,
                    endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                )
            },
            // Sin dispositivo de salida.
            Err(_) => (0.0, false),
        };
        Ok(SoundStatus {
            volume: volume.round(),
            muted,
            outputs: devices(&enumerator, eRender)?,
            inputs: devices(&enumerator, eCapture)?,
            startup_sound: reg::read_dword(BOOT_ANIMATION, "DisableStartupSound") != Some(1),
            system_sounds: reg::read_string(SCHEMES, "").as_deref() != Some(".None"),
        })
    })
}

pub fn set_volume(volume: f32) -> AppResult<()> {
    let level = (volume / 100.0).clamp(0.0, 1.0);
    com::mta(move || unsafe {
        endpoint_volume(&enumerator()?)?.SetMasterVolumeLevelScalar(level, std::ptr::null())?;
        Ok(())
    })
}

/// Usa este dispositivo (salida o entrada) como predeterminado para todo:
/// multimedia, comunicaciones y sonidos del sistema.
pub fn set_default_device(id: &str) -> AppResult<()> {
    let id = id.to_owned();
    com::mta(move || {
        let enumerator = enumerator()?;
        let device = devices(&enumerator, eRender)?
            .into_iter()
            .chain(devices(&enumerator, eCapture)?)
            .find(|device| device.id == id)
            .ok_or_else(|| AppError::not_found("Ese dispositivo ya no está conectado."))?;
        let policy: IPolicyConfig = unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)? };
        let wide = HSTRING::from(id.as_str());
        for role in [eConsole, eMultimedia, eCommunications] {
            set_default_endpoint(&policy, PCWSTR(wide.as_ptr()), role)?;
        }
        log::info!(target: "sonido", "Dispositivo predeterminado: {}", device.name);
        Ok(())
    })
}

pub fn set_mute(muted: bool) -> AppResult<()> {
    log::info!(target: "sonido", "{}", if muted { "Se silenció el sonido" } else { "Se reactivó el sonido" });
    com::mta(move || unsafe {
        endpoint_volume(&enumerator()?)?.SetMute(muted, std::ptr::null())?;
        Ok(())
    })
}

/// Activa el esquema "Predeterminado de Windows" o "Sin sonidos", como el panel clásico:
/// cambia el esquema y copia (o vacía) el sonido de cada evento.
fn set_system_sounds(on: bool) -> AppResult<()> {
    reg::write_string(SCHEMES, "", if on { ".Default" } else { ".None" })?;
    let apps = format!(r"{SCHEMES}\Apps");
    for app in reg::subkeys(&apps) {
        for event in reg::subkeys(&format!(r"{apps}\{app}")) {
            let base = format!(r"{apps}\{app}\{event}");
            let current = format!(r"{base}\.Current");
            if !reg::key_exists(&current) {
                continue;
            }
            let sound =
                if on { reg::read_string(&format!(r"{base}\.Default"), "").unwrap_or_default() } else { String::new() };
            reg::write_string(&current, "", &sound)?;
        }
    }
    Ok(())
}

pub fn set_flag(flag: SoundFlag, value: bool) -> AppResult<()> {
    log::info!(target: "sonido", "{flag:?}: {}", if value { "sí" } else { "no" });
    match flag {
        SoundFlag::StartupSound => {
            require_admin()?;
            reg::write_dword(BOOT_ANIMATION, "DisableStartupSound", (!value).into())
        }
        SoundFlag::SystemSounds => set_system_sounds(value),
    }
}

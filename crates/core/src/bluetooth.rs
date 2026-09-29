//! Bluetooth con WinRT: radio (encender/apagar) y dispositivos emparejados.

use serde::Serialize;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::com;

#[derive(Debug, Clone, Serialize)]
pub struct BluetoothDeviceInfo {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BluetoothStatus {
    pub available: bool,
    pub on: bool,
    pub devices: Vec<BluetoothDeviceInfo>,
}

fn bluetooth_radio() -> AppResult<Option<Radio>> {
    let radios = Radio::GetRadiosAsync()?.get()?;
    for index in 0..radios.Size()? {
        let radio = radios.GetAt(index)?;
        if radio.Kind()? == RadioKind::Bluetooth {
            return Ok(Some(radio));
        }
    }
    Ok(None)
}

fn paired(selector: &HSTRING, connected: impl Fn(&HSTRING) -> bool) -> AppResult<Vec<BluetoothDeviceInfo>> {
    let infos = DeviceInformation::FindAllAsyncAqsFilter(selector)?.get()?;
    let mut list = Vec::new();
    for index in 0..infos.Size()? {
        let info = infos.GetAt(index)?;
        let id = info.Id()?;
        list.push(BluetoothDeviceInfo {
            connected: connected(&id),
            name: info.Name()?.to_string(),
            id: id.to_string(),
        });
    }
    Ok(list)
}

fn read_status() -> AppResult<BluetoothStatus> {
    let Some(radio) = bluetooth_radio()? else {
        return Ok(BluetoothStatus { available: false, on: false, devices: Vec::new() });
    };
    let on = radio.State()? == RadioState::On;
    let is_connected = |status: windows::core::Result<BluetoothConnectionStatus>| {
        on && status.is_ok_and(|s| s == BluetoothConnectionStatus::Connected)
    };

    // Dispositivos clásicos (auriculares, parlantes) y de bajo consumo (mouses, teclados).
    let mut devices = paired(&BluetoothDevice::GetDeviceSelectorFromPairingState(true)?, |id| {
        is_connected(BluetoothDevice::FromIdAsync(id).and_then(|op| op.get()).and_then(|d| d.ConnectionStatus()))
    })?;
    for device in paired(&BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?, |id| {
        is_connected(BluetoothLEDevice::FromIdAsync(id).and_then(|op| op.get()).and_then(|d| d.ConnectionStatus()))
    })? {
        match devices.iter_mut().find(|d| d.name == device.name) {
            Some(existing) => existing.connected |= device.connected,
            None => devices.push(device),
        }
    }
    devices
        .sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(BluetoothStatus { available: true, on, devices })
}

pub fn status() -> AppResult<BluetoothStatus> {
    com::mta(read_status)
}

pub fn set_radio(on: bool) -> AppResult<()> {
    log::info!(target: "bluetooth", "Radio Bluetooth {}", if on { "encendida" } else { "apagada" });
    com::mta(move || {
        if Radio::RequestAccessAsync()?.get()? != RadioAccessStatus::Allowed {
            return Err(AppError::failed("Windows no permite controlar la radio Bluetooth."));
        }
        let radio = bluetooth_radio()?.ok_or_else(|| AppError::not_found("No hay un adaptador Bluetooth."))?;
        let status = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.get()?;
        if status == RadioAccessStatus::Allowed {
            Ok(())
        } else {
            Err(AppError::failed("No se pudo cambiar el Bluetooth."))
        }
    })
}

//! Red: adaptadores (IP, puerta de enlace, DNS, MAC) y servidores DNS.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::Serialize;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    DNS_INTERFACE_SETTINGS, DNS_INTERFACE_SETTINGS_VERSION1, DNS_SETTING_NAMESERVER, GAA_FLAG_INCLUDE_GATEWAYS,
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211,
    IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IP_ADAPTER_ADDRESSES_LH, SetInterfaceDnsSettings,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6, SOCKET_ADDRESS};
use windows::core::{GUID, PWSTR};

use crate::error::{AppError, AppResult};
use crate::util::win::require_admin;

/// IP_ADAPTER_DHCP_ENABLED dentro de `Flags`.
const DHCP_ENABLED: u32 = 0x4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AdapterKind {
    Ethernet,
    Wifi,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetAdapter {
    /// GUID del adaptador ("{...}").
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: AdapterKind,
    pub up: bool,
    pub mac: String,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub gateways: Vec<String>,
    pub dns: Vec<String>,
    pub dhcp: bool,
    pub speed_mbps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsPreset {
    Auto,
    Cloudflare,
    Google,
    Quad9,
    Adguard,
    OpenDns,
}

impl DnsPreset {
    pub const ALL: [DnsPreset; 6] =
        [Self::Auto, Self::Cloudflare, Self::Google, Self::Quad9, Self::Adguard, Self::OpenDns];

    pub fn servers(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Cloudflare => &["1.1.1.1", "1.0.0.1"],
            Self::Google => &["8.8.8.8", "8.8.4.4"],
            Self::Quad9 => &["9.9.9.9", "149.112.112.112"],
            Self::Adguard => &["94.140.14.14", "94.140.15.15"],
            Self::OpenDns => &["208.67.222.222", "208.67.220.220"],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Automático (DHCP)",
            Self::Cloudflare => "Cloudflare",
            Self::Google => "Google Public DNS",
            Self::Quad9 => "Quad9",
            Self::Adguard => "AdGuard DNS",
            Self::OpenDns => "OpenDNS",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Self::Auto => "El que asigna tu router o proveedor.",
            Self::Cloudflare => "Rápido y centrado en la privacidad.",
            Self::Google => "Estable y con amplia cobertura.",
            Self::Quad9 => "Bloquea dominios maliciosos.",
            Self::Adguard => "Bloquea anuncios y rastreadores.",
            Self::OpenDns => "De Cisco, con filtrado opcional.",
        }
    }

    /// Preset que coincide con los servidores en uso (por el primero).
    pub fn detect(dns: &[String]) -> DnsPreset {
        Self::ALL
            .into_iter()
            .find(|preset| preset.servers().first().is_some_and(|first| dns.first().is_some_and(|d| d == first)))
            .unwrap_or(Self::Auto)
    }
}

fn to_ip(address: &SOCKET_ADDRESS) -> Option<IpAddr> {
    if address.lpSockaddr.is_null() {
        return None;
    }
    unsafe {
        match (*address.lpSockaddr).sa_family {
            AF_INET => {
                let v4 = &*(address.lpSockaddr as *const SOCKADDR_IN);
                Some(IpAddr::V4(Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes())))
            }
            AF_INET6 => {
                let v6 = &*(address.lpSockaddr as *const SOCKADDR_IN6);
                Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.u.Byte)))
            }
            _ => None,
        }
    }
}

/// Recorre una lista enlazada de estructuras de IP Helper que tienen `Address` y `Next`.
macro_rules! collect_addresses {
    ($first:expr) => {{
        let mut ips = Vec::new();
        let mut node = $first;
        while !node.is_null() {
            let item = unsafe { &*node };
            if let Some(ip) = to_ip(&item.Address) {
                ips.push(ip);
            }
            node = item.Next;
        }
        ips
    }};
}

/// Adaptadores de red (sin loopback ni túneles).
pub fn adapters() -> AppResult<Vec<NetAdapter>> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = 16 * 1024u32;
    // Búfer de u64: respeta la alineación de las estructuras.
    let mut buffer: Vec<u64>;
    loop {
        buffer = vec![0; (size as usize).div_ceil(8)];
        let code = WIN32_ERROR(unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                flags,
                None,
                Some(buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        });
        if code == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if code != ERROR_SUCCESS {
            return Err(windows::core::Error::from(code.to_hresult()).into());
        }
        break;
    }

    let mut list = Vec::new();
    let mut node = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !node.is_null() {
        let adapter = unsafe { &*node };
        node = adapter.Next;
        if adapter.IfType == IF_TYPE_SOFTWARE_LOOPBACK || adapter.IfType == IF_TYPE_TUNNEL {
            continue;
        }
        let unicast = collect_addresses!(adapter.FirstUnicastAddress);
        let mac_len = (adapter.PhysicalAddressLength as usize).min(adapter.PhysicalAddress.len());
        let speed = adapter.TransmitLinkSpeed;
        list.push(NetAdapter {
            id: unsafe { adapter.AdapterName.to_string() }.unwrap_or_default(),
            name: unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default(),
            description: unsafe { adapter.Description.to_string() }.unwrap_or_default(),
            kind: match adapter.IfType {
                IF_TYPE_ETHERNET_CSMACD => AdapterKind::Ethernet,
                IF_TYPE_IEEE80211 => AdapterKind::Wifi,
                _ => AdapterKind::Other,
            },
            up: adapter.OperStatus == IfOperStatusUp,
            mac: adapter.PhysicalAddress[..mac_len].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"),
            ipv4: unicast.iter().filter(|ip| ip.is_ipv4()).map(ToString::to_string).collect(),
            ipv6: unicast.iter().filter(|ip| ip.is_ipv6()).map(ToString::to_string).collect(),
            gateways: collect_addresses!(adapter.FirstGatewayAddress).iter().map(ToString::to_string).collect(),
            dns: collect_addresses!(adapter.FirstDnsServerAddress).iter().map(ToString::to_string).collect(),
            dhcp: unsafe { adapter.Anonymous2.Flags } & DHCP_ENABLED != 0,
            speed_mbps: if speed == u64::MAX { 0 } else { speed / 1_000_000 },
        });
    }
    Ok(list)
}

/// Cambia los servidores DNS IPv4 de un adaptador (requiere administrador).
pub fn set_dns(adapter_id: &str, preset: DnsPreset) -> AppResult<()> {
    log::info!(target: "red", "DNS de {adapter_id}: {} {:?}", preset.name(), preset.servers());
    require_admin()?;
    if !adapters()?.iter().any(|a| a.id == adapter_id) {
        return Err(AppError::not_found("Ese adaptador de red ya no existe."));
    }
    let guid = GUID::try_from(adapter_id.trim_matches(['{', '}']))
        .map_err(|_| AppError::invalid("Identificador de adaptador inválido."))?;
    // Una lista vacía vuelve a los servidores que entrega DHCP.
    let mut servers: Vec<u16> = preset.servers().join(",").encode_utf16().chain([0]).collect();
    let settings = DNS_INTERFACE_SETTINGS {
        Version: DNS_INTERFACE_SETTINGS_VERSION1,
        Flags: DNS_SETTING_NAMESERVER as u64,
        NameServer: PWSTR(servers.as_mut_ptr()),
        ..Default::default()
    };
    let code = unsafe { SetInterfaceDnsSettings(guid, &settings) };
    if code == ERROR_SUCCESS { Ok(()) } else { Err(windows::core::Error::from(code.to_hresult()).into()) }
}

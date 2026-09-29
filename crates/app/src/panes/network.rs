//! Red: lista de adaptadores y detalle de cada uno (TCP/IP y servidores DNS).

use std::cell::RefCell;

use liquidcontrol_core::network::{self, AdapterKind, DnsPreset, NetAdapter};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::Pane;
use crate::toasts::{self, Kind};
use crate::{AdapterView, AppWindow, DnsView, Nav, NetworkState, bridge};

pub const PANE: Pane = Pane::new("network", |app, _| enter(app)).with_init(init).with_sub_title(sub_title);

thread_local! {
    static ADAPTERS: RefCell<Vec<NetAdapter>> = RefCell::default();
}

pub fn init(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<NetworkState>().on_set_dns(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&preset) = usize::try_from(index).ok().and_then(|i| DnsPreset::ALL.get(i)) else { return };
        let id = app.global::<Nav>().get_sub().to_string();
        let presets = app.global::<NetworkState>().get_presets();
        let Some(mut row) = presets.row_data(index as usize) else { return };
        if id.is_empty() || row.active || presets.iter().any(|p| p.busy) {
            return;
        }
        row.busy = true;
        presets.set_row_data(index as usize, row);
        bridge::spawn(
            &app,
            move || network::set_dns(&id, preset),
            |app, result| {
                match result {
                    Ok(()) => toasts::show(Kind::Success, "DNS actualizado."),
                    Err(err) => toasts::error(&err),
                }
                load(app);
            },
        );
    });
}

pub fn enter(app: &AppWindow) {
    load(app);
}

/// Título de la subpágina: el nombre del adaptador.
pub fn sub_title(sub: &str) -> Option<String> {
    ADAPTERS.with_borrow(|list| list.iter().find(|adapter| adapter.id == sub).map(|adapter| adapter.name.clone()))
}

fn load(app: &AppWindow) {
    bridge::spawn(app, network::adapters, |app, result| {
        match result {
            Ok(list) => {
                let views: Vec<AdapterView> = list.iter().map(adapter_view).collect();
                app.global::<NetworkState>().set_adapters(ModelRc::new(VecModel::from(views)));
                ADAPTERS.set(list);
                show_detail(app);
            }
            Err(err) => toasts::error(&err),
        }
        app.global::<NetworkState>().set_loaded(true);
    });
}

fn adapter_view(adapter: &NetAdapter) -> AdapterView {
    let detail = if adapter.up { adapter.ipv4.first().map_or("Conectado", String::as_str) } else { "Sin conexión" };
    AdapterView {
        id: adapter.id.as_str().into(),
        name: adapter.name.as_str().into(),
        detail: detail.into(),
        kind: match adapter.kind {
            AdapterKind::Ethernet => 0,
            AdapterKind::Wifi => 1,
            AdapterKind::Other => 2,
        },
        up: adapter.up,
    }
}

fn show_detail(app: &AppWindow) {
    let nav = app.global::<Nav>();
    let state = app.global::<NetworkState>();
    let sub = nav.get_sub();
    ADAPTERS.with_borrow(|list| {
        let Some(adapter) = list.iter().find(|adapter| adapter.id == sub.as_str()) else {
            state.set_detail_loaded(false);
            return;
        };
        if nav.get_pane() == "network" {
            nav.set_title(adapter.name.as_str().into());
        }
        let join = |items: &[String]| if items.is_empty() { "—".to_owned() } else { items.join(", ") };
        state.set_status(if adapter.up { "Conectado" } else { "Sin conexión" }.into());
        state.set_description(adapter.description.as_str().into());
        state.set_speed(speed(adapter.speed_mbps));
        state.set_mac(adapter.mac.as_str().into());
        state.set_config(if adapter.dhcp { "Automática (DHCP)" } else { "Manual" }.into());
        state.set_ipv4(join(&adapter.ipv4).into());
        state.set_gateway(join(&adapter.gateways).into());
        state.set_ipv6(adapter.ipv6.first().map(String::as_str).unwrap_or_default().into());
        state.set_dns_in_use(join(&adapter.dns).into());
        let active = DnsPreset::detect(&adapter.dns);
        let presets: Vec<DnsView> = DnsPreset::ALL
            .iter()
            .map(|preset| DnsView {
                name: preset.name().into(),
                note: preset.note().into(),
                server: preset.servers().first().copied().unwrap_or_default().into(),
                active: *preset == active,
                busy: false,
            })
            .collect();
        state.set_presets(ModelRc::new(VecModel::from(presets)));
        state.set_detail_loaded(true);
    });
}

/// Velocidad del enlace: "940 Mbps", "1 Gbps", "2,5 Gbps".
fn speed(mbps: u64) -> SharedString {
    match mbps {
        0 => SharedString::new(),
        m if m >= 1000 && m % 1000 == 0 => format!("{} Gbps", m / 1000).into(),
        m if m >= 1000 => format!("{:.1} Gbps", m as f64 / 1000.0).replace('.', ",").into(),
        m => format!("{m} Mbps").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::speed;

    #[test]
    fn link_speeds() {
        assert_eq!(speed(0), "");
        assert_eq!(speed(940), "940 Mbps");
        assert_eq!(speed(1000), "1 Gbps");
        assert_eq!(speed(2500), "2,5 Gbps");
    }
}

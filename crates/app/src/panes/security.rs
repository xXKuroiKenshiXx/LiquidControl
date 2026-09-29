//! Seguridad y antivirus: Windows Defender (protección en tiempo real y en la
//! nube, análisis, amenazas) y los tres perfiles del Firewall de Windows.

use liquidcontrol_core::actions;
use liquidcontrol_core::security::{self, FirewallProfile, FirewallStatus, ScanKind, SecurityStatus, Threat};
use slint::{ComponentHandle, Model};

use super::Pane;
use crate::toasts::{self, Kind};
use crate::{AppWindow, FirewallView, SecurityState, ThreatView, bridge, models, toasts as toasts_mod};

pub const PANE: Pane = Pane::new("security", |app, _| enter(app)).with_init(init);

fn init(app: &AppWindow) {
    let state = app.global::<SecurityState>();

    let weak = app.as_weak();
    state.on_set_realtime(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<SecurityState>().set_realtime_protection(on);
        bridge::spawn(
            &app,
            move || security::set_realtime_protection(on),
            move |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
                load(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_cloud(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<SecurityState>().set_cloud_protection(on);
        bridge::spawn(
            &app,
            move || security::set_cloud_protection(on),
            move |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
                load(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_firewall(move |profile, on| {
        let Some(app) = weak.upgrade() else { return };
        let Some(profile) = profile_from_index(profile) else { return };
        update_firewall(&app, profile, on);
        bridge::spawn(
            &app,
            move || security::set_firewall(profile, on),
            move |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_scan(move |full| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<SecurityState>().set_scanning(true);
        bridge::spawn(
            &app,
            move || security::start_scan(if full { ScanKind::Full } else { ScanKind::Quick }),
            |app, result| {
                app.global::<SecurityState>().set_scanning(false);
                match result {
                    Ok(()) => toasts_mod::show(
                        Kind::Info,
                        "Se inició el análisis: sigue en segundo plano, Windows te avisa si encuentra algo.",
                    ),
                    Err(err) => toasts::error(&err),
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_remove_threat(move |id| {
        let Some(app) = weak.upgrade() else { return };
        update_threat(&app, &id, |threat| threat.busy = true);
        let id = id.to_string();
        let task_id = id.clone();
        bridge::spawn(
            &app,
            move || security::remove_threat(&task_id),
            move |app, result| {
                match result {
                    Ok(()) => toasts_mod::show(Kind::Success, "Se quitó la amenaza."),
                    Err(err) => {
                        update_threat(app, &id, |threat| threat.busy = false);
                        toasts::error(&err);
                    }
                }
                load(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_open_security_app(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(
            &app,
            || actions::open_uri("windowsdefender:"),
            |_, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
            },
        );
    });
}

fn enter(app: &AppWindow) {
    load(app);
    load_threats(app);
}

fn load(app: &AppWindow) {
    bridge::spawn(app, security::status, |app, result| match result {
        Ok(status) => show(app, &status),
        Err(err) => toasts::error(&err),
    });
}

fn load_threats(app: &AppWindow) {
    bridge::spawn(app, security::threats, |app, result| match result {
        Ok(list) => show_threats(app, &list),
        Err(err) => toasts::error(&err),
    });
}

fn show(app: &AppWindow, status: &SecurityStatus) {
    let state = app.global::<SecurityState>();
    state.set_defender_installed(status.defender_installed);
    state.set_antivirus_enabled(status.antivirus_enabled);
    state.set_realtime_protection(status.realtime_protection);
    state.set_cloud_protection(status.cloud_protection);
    state.set_tamper_protection(status.tamper_protection);
    state.set_last_scan(status.last_scan.as_str().into());
    state.set_signature_age_days(status.signature_age_days as i32);
    let firewalls: Vec<FirewallView> = status.firewalls.iter().map(firewall_view).collect();
    state.set_firewalls(models::sync(state.get_firewalls(), firewalls));
    state.set_loaded(true);
}

fn show_threats(app: &AppWindow, list: &[Threat]) {
    let views: Vec<ThreatView> = list
        .iter()
        .map(|threat| ThreatView {
            id: threat.id.as_str().into(),
            name: threat.name.as_str().into(),
            path: threat.path.as_str().into(),
            severity: threat.severity as i32,
            detected: threat.detected.as_str().into(),
            busy: false,
        })
        .collect();
    let state = app.global::<SecurityState>();
    state.set_threats(models::sync(state.get_threats(), views));
}

fn firewall_view(status: &FirewallStatus) -> FirewallView {
    FirewallView {
        profile: profile_index(status.profile),
        label: status.profile.label().into(),
        enabled: status.enabled,
    }
}

fn profile_index(profile: FirewallProfile) -> i32 {
    match profile {
        FirewallProfile::Domain => 0,
        FirewallProfile::Private => 1,
        FirewallProfile::Public => 2,
    }
}

fn profile_from_index(index: i32) -> Option<FirewallProfile> {
    match index {
        0 => Some(FirewallProfile::Domain),
        1 => Some(FirewallProfile::Private),
        2 => Some(FirewallProfile::Public),
        _ => None,
    }
}

fn update_firewall(app: &AppWindow, profile: FirewallProfile, on: bool) {
    let model = app.global::<SecurityState>().get_firewalls();
    let index = profile_index(profile);
    if let Some((row, mut view)) = model.iter().enumerate().find(|(_, view)| view.profile == index) {
        view.enabled = on;
        model.set_row_data(row, view);
    }
}

fn update_threat(app: &AppWindow, id: &str, change: impl FnOnce(&mut ThreatView)) {
    let model = app.global::<SecurityState>().get_threats();
    if let Some((row, mut view)) = model.iter().enumerate().find(|(_, view)| view.id == id) {
        change(&mut view);
        model.set_row_data(row, view);
    }
}

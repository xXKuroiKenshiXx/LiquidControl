//! Energía: batería, planes de energía (elegir, crear, renombrar, borrar y
//! restaurar los de fábrica), tiempos de espera e hibernación.

use std::cell::RefCell;
use std::time::Duration;

use liquidcontrol_core::power::{self, PowerSource, PowerStatus, TimeoutKind, ULTIMATE_PLAN};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::Pane;
use crate::toasts::{self, Kind};
use crate::{AppWindow, EnergyState, Nav, PlanView, bridge, dialogs, format, models};

pub const PANE: Pane = Pane::new("energy", |app, _| load(app)).with_init(init);

/// Opciones de los menús de tiempo de espera (0 = nunca).
const MINUTES: [u32; 14] = [1, 2, 3, 5, 10, 15, 20, 30, 45, 60, 120, 180, 300, 0];
const BATTERY_REFRESH: Duration = Duration::from_secs(30);
const NEW_DIALOG: &str = "plan-new";
const RENAME_DIALOG: &str = "plan-rename";
const DELETE_DIALOG: &str = "plan-delete";
const RESTORE_DIALOG: &str = "plan-restore";

thread_local! {
    static REFRESH: Timer = Timer::default();
    /// Plan que se está por renombrar o borrar: (guid, nombre).
    static TARGET: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

fn init(app: &AppWindow) {
    let state = app.global::<EnergyState>();
    let labels: Vec<SharedString> = MINUTES.iter().map(|minutes| minutes_label(*minutes).into()).collect();
    state.set_minute_labels(ModelRc::new(VecModel::from(labels)));

    let weak = app.as_weak();
    state.on_set_plan(move |guid| {
        let Some(app) = weak.upgrade() else { return };
        let plans = app.global::<EnergyState>().get_plans();
        for (index, mut plan) in plans.iter().enumerate() {
            plan.active = plan.guid == guid;
            plans.set_row_data(index, plan);
        }
        let guid = guid.to_string();
        bridge::spawn(
            &app,
            move || power::set_plan(&guid),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                }
                load(app);
            },
        );
    });

    let weak = app.as_weak();
    state.on_add_ultimate(move || {
        let Some(app) = weak.upgrade() else { return };
        bridge::spawn(&app, power::add_ultimate, |app, result| {
            match result {
                Ok(()) => toasts::show(Kind::Success, "Se agregó y activó el plan «Máximo rendimiento»."),
                Err(err) => toasts::error(&err),
            }
            load(app);
        });
    });

    init_plan_dialogs(app);

    let weak = app.as_weak();
    state.on_set_timeout(move |key, index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&minutes) = usize::try_from(index).ok().and_then(|i| MINUTES.get(i)) else { return };
        let (kind, source) = match key.as_str() {
            "screen-ac" => (TimeoutKind::Screen, PowerSource::Ac),
            "screen-dc" => (TimeoutKind::Screen, PowerSource::Dc),
            "sleep-ac" => (TimeoutKind::Sleep, PowerSource::Ac),
            "sleep-dc" => (TimeoutKind::Sleep, PowerSource::Dc),
            _ => return,
        };
        show_timeout(&app, kind, source, index);
        bridge::spawn(
            &app,
            move || power::set_timeout(kind, source, minutes),
            |app, result| {
                if let Err(err) = result {
                    toasts::error(&err);
                    load(app);
                }
            },
        );
    });

    let weak = app.as_weak();
    state.on_set_hibernate(move |on| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<EnergyState>().set_hibernate(on);
        bridge::spawn(
            &app,
            move || power::set_hibernate(on),
            move |app, result| {
                if let Err(err) = result {
                    app.global::<EnergyState>().set_hibernate(!on);
                    toasts::error(&err);
                }
            },
        );
    });

    // La batería se actualiza sola mientras el panel está a la vista.
    let weak = app.as_weak();
    REFRESH.with(|timer| {
        timer.start(TimerMode::Repeated, BATTERY_REFRESH, move || {
            let Some(app) = weak.upgrade() else { return };
            if app.global::<Nav>().get_pane() == "energy" && app.global::<EnergyState>().get_has_battery() {
                load(&app);
            }
        });
    });
}

/// Crear, renombrar, borrar y restaurar planes (cada uno con su diálogo).
fn init_plan_dialogs(app: &AppWindow) {
    let state = app.global::<EnergyState>();

    let weak = app.as_weak();
    state.on_new_plan(move || {
        let Some(app) = weak.upgrade() else { return };
        dialogs::ask_text(
            &app,
            NEW_DIALOG,
            "Nuevo plan de energía",
            "Se crea a partir del plan en uso, con sus mismos tiempos de espera, y queda activado.",
            "Nombre del plan",
            "",
            "Crear",
        );
    });
    dialogs::register(NEW_DIALOG, |app, name| {
        let name = name.to_string();
        dialogs::set_busy(app, true);
        bridge::spawn(
            app,
            move || power::create_plan(&name).map(|_| name),
            |app, result| match result {
                Ok(name) => {
                    dialogs::close(app);
                    toasts::show(Kind::Success, format!("Se creó el plan «{name}»."));
                    load(app);
                }
                Err(err) => dialogs::set_error(app, &err.to_string()),
            },
        );
    });

    let weak = app.as_weak();
    state.on_rename_plan(move |guid| {
        let Some(app) = weak.upgrade() else { return };
        let Some(name) = plan_name(&app, &guid) else { return };
        TARGET.set(Some((guid.to_string(), name.clone())));
        dialogs::ask_text(&app, RENAME_DIALOG, "Renombrar el plan", "", "Nombre del plan", &name, "Renombrar");
    });
    dialogs::register(RENAME_DIALOG, |app, name| {
        // Se conserva el plan elegido hasta que salga bien: si falla, se puede corregir el nombre.
        let Some((guid, _)) = TARGET.with_borrow(Clone::clone) else {
            dialogs::close(app);
            return;
        };
        let name = name.to_string();
        dialogs::set_busy(app, true);
        bridge::spawn(
            app,
            move || power::rename_plan(&guid, &name),
            |app, result| match result {
                Ok(()) => {
                    TARGET.set(None);
                    dialogs::close(app);
                    load(app);
                }
                Err(err) => dialogs::set_error(app, &err.to_string()),
            },
        );
    });
    dialogs::register_cancel(RENAME_DIALOG, forget_target);

    let weak = app.as_weak();
    state.on_delete_plan(move |guid| {
        let Some(app) = weak.upgrade() else { return };
        let Some(name) = plan_name(&app, &guid) else { return };
        TARGET.set(Some((guid.to_string(), name.clone())));
        dialogs::confirm(
            &app,
            DELETE_DIALOG,
            &format!("¿Eliminar el plan «{name}»?"),
            "Se borra con todos sus ajustes. Los planes de fábrica se recuperan con «Restaurar planes de fábrica».",
            "Eliminar",
            true,
        );
    });
    dialogs::register(DELETE_DIALOG, |app, _| {
        let Some((guid, name)) = TARGET.take() else {
            dialogs::close(app);
            return;
        };
        dialogs::set_busy(app, true);
        bridge::spawn(
            app,
            move || power::delete_plan(&guid),
            move |app, result| {
                dialogs::close(app);
                match result {
                    Ok(()) => toasts::show(Kind::Success, format!("Se eliminó el plan «{name}».")),
                    Err(err) => toasts::error(&err),
                }
                load(app);
            },
        );
    });
    dialogs::register_cancel(DELETE_DIALOG, forget_target);

    let weak = app.as_weak();
    state.on_restore_plans(move || {
        let Some(app) = weak.upgrade() else { return };
        dialogs::confirm(
            &app,
            RESTORE_DIALOG,
            "¿Restaurar los planes de fábrica?",
            "Se borran los planes personalizados (también «Máximo rendimiento» si lo agregaste) y vuelven los de Windows con sus valores originales.",
            "Restaurar",
            true,
        );
    });
    dialogs::register(RESTORE_DIALOG, |app, _| {
        dialogs::set_busy(app, true);
        bridge::spawn(app, power::restore_default_plans, |app, result| {
            dialogs::close(app);
            match result {
                Ok(()) => toasts::show(Kind::Success, "Se restauraron los planes de fábrica."),
                Err(err) => toasts::error(&err),
            }
            load(app);
        });
    });
}

fn forget_target(app: &AppWindow) {
    TARGET.set(None);
    dialogs::close(app);
}

fn plan_name(app: &AppWindow, guid: &str) -> Option<String> {
    app.global::<EnergyState>().get_plans().iter().find(|plan| plan.guid == guid).map(|plan| plan.name.to_string())
}

fn load(app: &AppWindow) {
    bridge::spawn(app, power::status, |app, result| match result {
        Ok(status) => show(app, &status),
        Err(err) => toasts::error(&err),
    });
}

fn show(app: &AppWindow, status: &PowerStatus) {
    let state = app.global::<EnergyState>();
    let percent = status.battery_percent.filter(|_| status.has_battery);
    state.set_has_battery(percent.is_some());
    state.set_battery(percent.map_or(0, i32::from));
    state.set_battery_state(battery_state(status).into());
    let plans: Vec<PlanView> = status
        .plans
        .iter()
        .map(|plan| PlanView {
            guid: plan.guid.as_str().into(),
            name: plan.name.as_str().into(),
            active: plan.active,
            ultimate: is_ultimate(&plan.guid, &plan.name),
        })
        .collect();
    state.set_has_ultimate(plans.iter().any(|plan| plan.ultimate));
    state.set_plans(models::sync(state.get_plans(), plans));
    state.set_hibernate(status.hibernate);
    state.set_screen_ac(option_index(status.screen_off_ac));
    state.set_screen_dc(option_index(status.screen_off_dc));
    state.set_sleep_ac(option_index(status.sleep_ac));
    state.set_sleep_dc(option_index(status.sleep_dc));
    state.set_loaded(true);
}

fn show_timeout(app: &AppWindow, kind: TimeoutKind, source: PowerSource, index: i32) {
    let state = app.global::<EnergyState>();
    match (kind, source) {
        (TimeoutKind::Screen, PowerSource::Ac) => state.set_screen_ac(index),
        (TimeoutKind::Screen, PowerSource::Dc) => state.set_screen_dc(index),
        (TimeoutKind::Sleep, PowerSource::Ac) => state.set_sleep_ac(index),
        (TimeoutKind::Sleep, PowerSource::Dc) => state.set_sleep_dc(index),
    }
}

fn battery_state(status: &PowerStatus) -> String {
    if status.charging {
        "Cargando".into()
    } else if status.on_ac {
        "Conectada a la corriente".into()
    } else if let Some(seconds) = status.seconds_left {
        format!("{} restantes", format::duration(u64::from(seconds)))
    } else {
        "Con batería".into()
    }
}

/// El plan oculto "Máximo rendimiento" (o una copia suya, que tiene otro GUID).
fn is_ultimate(guid: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    guid.eq_ignore_ascii_case(ULTIMATE_PLAN) || name.contains("máximo") || name.contains("ultimate")
}

fn minutes_label(minutes: u32) -> String {
    match minutes {
        0 => "Nunca".into(),
        1 => "1 minuto".into(),
        60 => "1 hora".into(),
        m if m < 60 => format!("{m} minutos"),
        m => format!("{} horas", m / 60),
    }
}

/// Opción del menú para un valor del sistema; si no coincide, la más cercana.
fn option_index(minutes: u32) -> i32 {
    let index = MINUTES.iter().position(|m| *m == minutes).unwrap_or_else(|| {
        (0..MINUTES.len()).filter(|i| MINUTES[*i] != 0).min_by_key(|i| MINUTES[*i].abs_diff(minutes)).unwrap_or(0)
    });
    index as i32
}

#[cfg(test)]
mod tests {
    use super::{MINUTES, is_ultimate, minutes_label, option_index};

    #[test]
    fn labels_and_nearest_option() {
        assert_eq!(minutes_label(0), "Nunca");
        assert_eq!(minutes_label(45), "45 minutos");
        assert_eq!(minutes_label(180), "3 horas");
        assert_eq!(MINUTES[option_index(0) as usize], 0);
        assert_eq!(MINUTES[option_index(12) as usize], 10);
        assert_eq!(MINUTES[option_index(1000) as usize], 300);
    }

    #[test]
    fn recognizes_ultimate_copies() {
        assert!(is_ultimate("E9A42B02-D5DF-448D-AA00-03F14749EB61", "x"));
        assert!(is_ultimate("otro", "Máximo rendimiento"));
        assert!(!is_ultimate("otro", "Equilibrado"));
    }
}

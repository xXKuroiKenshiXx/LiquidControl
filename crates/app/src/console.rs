//! Consola de actividad: muestra en vivo el registro de `liquidcontrol_core::activity`
//! (qué se hizo, cómo y si funcionó) sin abrir ninguna consola aparte.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use liquidcontrol_core::actions;
use liquidcontrol_core::activity::{self, Entry, Level};
use liquidcontrol_core::util::clipboard;
use slint::{ComponentHandle, FilterModel, Model, ModelRc, VecModel};

use crate::toasts::{self, Kind};
use crate::{AppWindow, ConsoleState, LogEntryView, bridge};

const MAX_VISIBLE: usize = 500;

type Filtered = FilterModel<Rc<VecModel<LogEntryView>>, Box<dyn Fn(&LogEntryView) -> bool>>;

thread_local! {
    /// De la más nueva a la más vieja.
    static ENTRIES: Rc<VecModel<LogEntryView>> = Rc::new(VecModel::default());
    static ONLY_PROBLEMS: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    static FILTERED: RefCell<Option<Rc<Filtered>>> = const { RefCell::new(None) };
}

fn level_code(level: Level) -> i32 {
    match level {
        Level::Info => 0,
        Level::Success => 1,
        Level::Warning => 2,
        Level::Error => 3,
    }
}

fn view(entry: &Entry) -> LogEntryView {
    LogEntryView {
        time: entry.time.as_str().into(),
        level: level_code(entry.level),
        area: entry.area.as_str().into(),
        message: entry.message.as_str().into(),
    }
}

pub fn init(app: &AppWindow) {
    let state = app.global::<ConsoleState>();
    let entries = ENTRIES.with(Rc::clone);
    for entry in activity::entries().iter().rev() {
        entries.push(view(entry));
    }
    let only_problems = ONLY_PROBLEMS.with(Rc::clone);
    let filter: Box<dyn Fn(&LogEntryView) -> bool> = Box::new(move |entry| !only_problems.get() || entry.level >= 2);
    let filtered = Rc::new(FilterModel::new(entries, filter));
    state.set_entries(ModelRc::from(filtered.clone()));
    FILTERED.set(Some(filtered));
    state.set_file(activity::log_file().map(|path| path.display().to_string()).unwrap_or_default().into());

    // Las entradas pueden llegar desde cualquier hilo: se pasan al de la interfaz.
    let weak = app.as_weak();
    activity::set_listener(move |entry| {
        let entry = view(entry);
        let _ = weak.upgrade_in_event_loop(move |_| push(entry));
    });

    state.on_set_filter({
        let weak = app.as_weak();
        move |index| {
            let Some(app) = weak.upgrade() else { return };
            ONLY_PROBLEMS.with(|only| only.set(index == 1));
            app.global::<ConsoleState>().set_filter(index);
            FILTERED.with_borrow(|filtered| {
                if let Some(filtered) = filtered {
                    filtered.reset();
                }
            });
        }
    });

    state.on_copy({
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let text: String = activity::entries()
                .iter()
                .map(|entry| format!("{} [{:?}] {}: {}\n", entry.time, entry.level, entry.area, entry.message))
                .collect();
            bridge::spawn(
                &app,
                move || clipboard::set_text(&text),
                |_, result| match result {
                    Ok(()) => toasts::show(Kind::Success, "Se copió el registro al portapapeles."),
                    Err(err) => toasts::error(&err),
                },
            );
        }
    });

    state.on_clear(|| {
        activity::clear();
        ENTRIES.with(|entries| entries.set_vec(Vec::new()));
    });

    state.on_open_folder({
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(folder) = activity::log_file().and_then(|file| file.parent().map(ToOwned::to_owned)) else {
                return;
            };
            bridge::spawn(
                &app,
                move || actions::open_folder(&folder),
                |_, result| {
                    if let Err(err) = result {
                        toasts::error(&err);
                    }
                },
            );
        }
    });
}

fn push(entry: LogEntryView) {
    ENTRIES.with(|entries| {
        entries.insert(0, entry);
        if entries.row_count() > MAX_VISIBLE {
            entries.remove(entries.row_count() - 1);
        }
    });
}

//! Headless windows and polling shared by the `*_ui_tests` modules.

use super::agent::AgentStore;
use super::*;
use crate::settings::Settings;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root, test::TestWindowExt};

/// Fake time per poll: views poll background work on 50 ms timers.
pub(super) const TICK: Duration = Duration::from_millis(50);
/// Polls before giving up. With `PAUSE` this allows at least 4 s of real time, enough for real
/// Git calls, a shell on a pty and the fake agent child process.
const TRIES: usize = 400;
const PAUSE: Duration = Duration::from_millis(10);

pub(super) fn empty_store() -> AgentStore {
    AgentStore {
        history: None,
        default_workspace: None,
    }
}

/// Initializes Kit and sets the globals a window needs; returns the shared documents.
pub(super) fn install_globals(
    cx: &mut App,
    settings: Settings,
    store: AgentStore,
) -> DocumentOwners {
    gpui_kit::init(cx);
    cx.set_app_identity("local.zj.editor", "ZJ");
    super::init_agent_notifications(cx);
    cx.set_global(settings);
    cx.set_global(crate::watch::WatchService::default());
    cx.set_global(store);
    let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
    cx.set_global(OpenDocuments(documents.clone()));
    documents
}

/// Opens one workbench window with its own Git service; globals must already be set.
pub(super) fn new_window(
    cx: &mut App,
    root: Option<PathBuf>,
    documents: DocumentOwners,
    bounds: Bounds<Pixels>,
) -> (AnyWindowHandle, Entity<Workbench>) {
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| Workbench::new(root, service, documents, 1, window, cx)),
    )
    .unwrap()
}

/// Sets the globals and opens a 1400×900 window, without waiting for it to load.
pub(super) fn open_window(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
    settings: Settings,
    store: AgentStore,
) -> (WindowHandle<Root>, Entity<Workbench>) {
    cx.update(|cx| {
        let documents = install_globals(cx, settings, store);
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        let (window, this) = new_window(cx, root, documents, bounds);
        (window.downcast::<Root>().unwrap(), this)
    })
}

/// Opens a window on `root` with default settings and waits until the workspace has loaded.
pub(super) fn open(
    cx: &mut TestAppContext,
    root: PathBuf,
) -> (WindowHandle<Root>, Entity<Workbench>) {
    let (window, this) = open_window(cx, Some(root), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the repository never finished loading".into(),
    );
    (window, this)
}

pub(super) fn loaded(cx: &mut TestAppContext, this: &Entity<Workbench>) -> bool {
    this.read_with(cx, |p, _| p.refresh_completed && !p.loading)
}

/// Advances fake time, renders `window` if given, and polls until `done` holds.
pub(super) fn settle(
    cx: &mut TestAppContext,
    window: Option<WindowHandle<Root>>,
    done: impl FnMut(&mut TestAppContext) -> bool,
) {
    wait(cx, Some(TICK), window, done, |_| {
        "the operation never settled".into()
    });
}

/// The polling loop behind `settle`: `advance` is the fake time per poll (`None` leaves the clock
/// alone), and `diagnose` builds the panic message on timeout.
pub(super) fn wait(
    cx: &mut TestAppContext,
    advance: Option<Duration>,
    window: Option<WindowHandle<Root>>,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
    diagnose: impl FnOnce(&mut TestAppContext) -> String,
) {
    for _ in 0..TRIES {
        if let Some(advance) = advance {
            cx.executor().advance_clock(advance);
        }
        cx.run_until_parked();
        if let Some(window) = window {
            cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
        if done(cx) {
            return;
        }
        std::thread::sleep(PAUSE);
    }
    panic!("{}", diagnose(cx));
}

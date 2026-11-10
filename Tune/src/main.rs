mod api;
mod backend;
mod keyring;
mod lyrics;
mod mpris;
mod settings;
mod ui;

use std::future::Future;
use std::sync::OnceLock;

use adw::prelude::*;

pub const APP_ID: &str = "io.github.rice.Tune";

/// librespot, the keyring and HTTP all run on tokio; GTK runs on its own
/// main loop. `tokio()` hands a job over and lets GTK await the result.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tune-tokio")
            .build()
            .expect("tokio runtime")
    })
}

pub async fn tokio<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future).await.expect("tokio task panicked")
}

fn main() -> gtk::glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,tune=info")).init();

    // Name the audio stream in GNOME's sound settings and volume mixer.
    // Set before any thread exists, so nothing else can be reading the environment.
    unsafe {
        std::env::set_var("PULSE_PROP_application.name", "Tune");
        std::env::set_var("PULSE_PROP_application.icon_name", APP_ID);
        std::env::set_var("PULSE_PROP_media.role", "music");
    }
    let _ = runtime();

    let mut flags = gtk::gio::ApplicationFlags::empty();
    // Snapshot runs (see ui.rs) start beside a Tune that is already open.
    if std::env::var_os("TUNE_SNAPSHOT_DIR").is_some() {
        flags |= gtk::gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_activate(ui::activate);
    app.run()
}

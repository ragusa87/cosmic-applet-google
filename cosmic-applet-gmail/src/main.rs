mod app;
mod config;
mod gmail;
mod settings;
mod ui;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,cosmic_applet_gmail=info")),
        )
        .init();

    if std::env::args().any(|arg| arg == "--show-settings") {
        settings::run()
    } else {
        cosmic::applet::run::<app::AppModel>(())
    }
}

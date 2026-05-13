mod app;
mod config;
mod export;
mod lock;
mod sessions;
mod settings;
mod state;
mod taxi;
mod ui;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,cosmic_applet_taxi=info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--show-settings") {
        settings::run()
    } else if args.iter().any(|a| a == "--show-export") {
        export::run()
    } else {
        cosmic::applet::run::<app::AppModel>(())
    }
}

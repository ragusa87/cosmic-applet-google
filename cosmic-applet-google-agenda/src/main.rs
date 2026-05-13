mod app;
mod auth;
mod calendar;
mod config;
mod debug;
mod secrets;
mod settings;
mod ui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("warn,cosmic_applet_google_agenda=info")
            }),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let has_flag = |name: &str| args.iter().any(|a| a == name);

    if has_flag("--notify") {
        fire_test_notification()?;
    }
    if has_flag("--debug") {
        debug::run()?;
        return Ok(());
    }
    if has_flag("--notify") {
        return Ok(());
    }
    if has_flag("--show-settings") {
        settings::run()?;
        return Ok(());
    }
    cosmic::applet::run::<app::AppModel>(())?;
    Ok(())
}

fn fire_test_notification() -> Result<(), Box<dyn std::error::Error>> {
    notify_rust::Notification::new()
        .summary("Test notification")
        .body("cosmic-applet-google-agenda \u{2014} test notification")
        .icon(config::APP_ID)
        .show()?;
    Ok(())
}

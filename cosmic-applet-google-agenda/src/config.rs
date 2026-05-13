use cosmic_config::CosmicConfigEntry;
use cosmic_config_derive::CosmicConfigEntry;

pub const APP_ID: &str = "com.github.ragusa87.CosmicAppletGoogleAgenda";

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    pub email: String,
    pub client_id: String,
    pub fetch_interval_secs: u32,
    pub display_tick_secs: u32,
    pub notification_lead_secs: u32,
    pub notify: bool,
    pub show_title: bool,
    pub show_time: bool,
    pub show_progress: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            email: String::new(),
            client_id: String::new(),
            fetch_interval_secs: 300,
            display_tick_secs: 30,
            notification_lead_secs: 300,
            notify: true,
            show_title: true,
            show_time: true,
            show_progress: true,
        }
    }
}

impl Config {
    pub fn fetch_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.fetch_interval_secs.max(60)))
    }

    pub fn display_tick(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.display_tick_secs.max(5)))
    }

    pub fn is_configured(&self) -> bool {
        !self.email.is_empty() && !self.client_id.is_empty()
    }
}

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use cosmic_config::CosmicConfigEntry;

use crate::auth;
use crate::calendar::{self, DebugItem};
use crate::config::{APP_ID, Config};
use crate::secrets;

pub fn run() -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(run_async())
}

#[allow(clippy::too_many_lines)]
async fn run_async() -> Result<()> {
    println!("=== cosmic-applet-google-agenda --debug ===");
    println!();

    let config = cosmic_config::Config::new(APP_ID, Config::VERSION)
        .map(|ctx| match Config::get_entry(&ctx) {
            Ok(c) => c,
            Err((_errors, c)) => c,
        })
        .context("open cosmic-config")?;

    println!("Config (~/.config/{APP_ID}/v{}):", Config::VERSION);
    println!("  email:                  {}", show_string(&config.email));
    println!(
        "  client_id:              {}",
        show_string(&config.client_id)
    );
    println!("  fetch_interval_secs:    {}", config.fetch_interval_secs);
    println!("  display_tick_secs:      {}", config.display_tick_secs);
    println!(
        "  notification_lead_secs: {}",
        config.notification_lead_secs
    );
    println!("  notify:                 {}", config.notify);
    println!("  show_title:             {}", config.show_title);
    println!("  show_time:              {}", config.show_time);
    println!("  show_progress:          {}", config.show_progress);
    println!();

    if !config.is_configured() {
        println!("Applet is not configured. Run `--show-settings` first.");
        return Ok(());
    }

    println!("Loading tokens from Secret Service for {}...", config.email);
    let tokens = match secrets::load(&config.email).await {
        Ok(t) => t,
        Err(e) => {
            println!("  failed: {e}");
            return Ok(());
        }
    };
    let now_unix = unix_now();
    println!(
        "  refresh_token:          {}",
        if tokens.refresh_token.is_empty() {
            "(EMPTY — applet would not work)"
        } else {
            "(present)"
        }
    );
    print!("  access_token:           ");
    if tokens.expires_at_unix > now_unix {
        println!(
            "(present, expires in {}s)",
            tokens.expires_at_unix - now_unix
        );
    } else {
        println!(
            "(present, EXPIRED {}s ago)",
            now_unix.saturating_sub(tokens.expires_at_unix)
        );
    }
    println!();

    let tokens = if tokens.is_access_token_fresh() {
        println!("Access token is fresh — no refresh needed.");
        tokens
    } else {
        println!("Access token is stale, refreshing...");
        let new = auth::refresh(&config.client_id, &tokens)
            .await
            .context("refresh access token")?;
        println!(
            "  refreshed (new expiry in {}s)",
            new.expires_at_unix.saturating_sub(unix_now())
        );
        new
    };
    println!();

    println!("Fetching events from primary calendar (now → now + 24h)...");
    let items = calendar::debug_fetch(&tokens.access_token)
        .await
        .context("debug_fetch")?;
    println!("Received {} event(s).", items.len());
    println!();

    let mut kept_by_id: BTreeMap<chrono::DateTime<chrono::Utc>, &DebugItem> = BTreeMap::new();
    let mut skip_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut kept = 0usize;

    for (idx, item) in items.iter().enumerate() {
        print_item(idx + 1, item);
        match &item.verdict {
            Ok(ev) => {
                kept += 1;
                kept_by_id.insert(ev.start, item);
            }
            Err(reason) => {
                *skip_counts.entry(reason.to_string()).or_insert(0) += 1;
            }
        }
    }

    println!("Summary:");
    println!("  total received: {}", items.len());
    println!("  kept:           {kept}");
    for (reason, count) in &skip_counts {
        println!("  skipped:        {count} — {reason}");
    }
    println!();

    if let Some((_, item)) = kept_by_id.iter().next()
        && let Ok(ev) = &item.verdict
    {
        let delta = (ev.start - chrono::Utc::now()).num_seconds();
        let until = if delta <= 0 {
            "in progress (now)".to_owned()
        } else {
            format!(
                "in {}m{}s",
                delta.div_euclid(60),
                delta.rem_euclid(60)
            )
        };
        println!("Next visible event: {} — starts {}", ev.summary, until);
        let lead = i64::from(config.notification_lead_secs);
        if !config.notify {
            println!("Notifications disabled (notify = false).");
        } else if config.notification_lead_secs == 0 {
            println!("Notifications disabled (notification_lead_secs = 0).");
        } else if delta < 0 {
            println!("No notification — event already started.");
        } else if delta <= lead {
            println!(
                "Notification would fire NOW (event is within lead window of {lead}s)."
            );
        } else {
            println!(
                "Notification would fire in {}s (lead = {lead}s).",
                delta - lead
            );
        }
    } else {
        println!("No visible event in the next 24h.");
    }

    Ok(())
}

fn print_item(index: usize, item: &DebugItem) {
    println!("[{index}] {}", item.summary);
    println!("    id:           {}", item.id);
    println!("    start:        {}", item.start_display);
    println!("    end:          {}", item.end_display);
    if let Some(s) = &item.status {
        println!("    status:       {s}");
    }
    if let Some(t) = &item.transparency {
        println!("    transparency: {t}");
    }
    if item.attendee_count > 0 {
        let self_part = item
            .self_response
            .as_deref()
            .map(|r| format!(" (self: {r})"))
            .unwrap_or_default();
        println!(
            "    attendees:    {}{}",
            item.attendee_count, self_part
        );
    }
    if let Some(m) = &item.meet_url {
        println!("    meet:         {m}");
    }
    if let Some(l) = &item.location {
        println!("    location:     {l}");
    }
    match &item.verdict {
        Ok(_) => println!("    VERDICT:      KEEP"),
        Err(reason) => println!("    VERDICT:      SKIP — {reason}"),
    }
    println!();
}

fn show_string(s: &str) -> &str {
    if s.is_empty() { "(empty)" } else { s }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

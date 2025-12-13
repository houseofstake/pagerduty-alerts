//! Main entry point for the NEAR PagerDuty Monitor binary

use near_pagerduty_alerts::house_of_stake_config;
use near_pagerduty_alerts::PagerDutyAlertConfig;
use std::path::Path;
// Uncomment as needed:
// use near_pagerduty_alerts::{contract_events_config, transaction_monitor_config};

fn load_config_from_file(path: &str) -> Result<PagerDutyAlertConfig, anyhow::Error> {
    let content = std::fs::read_to_string(path)?;
    let mut config: PagerDutyAlertConfig = serde_yaml::from_str(&content)?;

    // If routing key is not in config file, get it from environment variable
    if config.routing_key.is_empty() {
        config.routing_key = std::env::var("PAGERDUTY_ROUTING_KEY")
            .map_err(|_| anyhow::anyhow!(
                "PAGERDUTY_ROUTING_KEY must be set either in config.yaml or as an environment variable"
            ))?;
        log::info!("Using PAGERDUTY_ROUTING_KEY from environment variable");
    }

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    // Try to load config from config.yaml, fallback to environment variable + hardcoded config
    let config = if Path::new("config.yaml").exists() {
        log::info!("Loading configuration from config.yaml");
        load_config_from_file("config.yaml")?
    } else if Path::new("rust/config.yaml").exists() {
        log::info!("Loading configuration from rust/config.yaml");
        load_config_from_file("rust/config.yaml")?
    } else {
        log::info!("No config.yaml found, using hardcoded House of Stake configuration");
        let routing_key = std::env::var("PAGERDUTY_ROUTING_KEY").expect(
            "PAGERDUTY_ROUTING_KEY environment variable required when no config.yaml is present",
        );

        // Choose your configuration:
        house_of_stake_config(&routing_key)

        // Or monitor a custom contract:
        // contract_events_config(&routing_key, "your-contract.near", Some("nep141"))

        // Or monitor transactions:
        // transaction_monitor_config(&routing_key, "your-contract.near")
    };

    log::info!(
        "Starting NEAR event monitor with {} subscription(s)",
        config.subscriptions.len()
    );

    let monitor = near_pagerduty_alerts::NearPagerDutyMonitor::new(config);
    monitor.start().await?;

    Ok(())
}

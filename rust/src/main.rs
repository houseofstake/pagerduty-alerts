//! Main entry point for the NEAR PagerDuty Monitor binary

use near_pagerduty_alerts::house_of_stake_config;
// Uncomment as needed:
// use near_pagerduty_alerts::{contract_events_config, transaction_monitor_config};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    let routing_key = std::env::var("PAGERDUTY_ROUTING_KEY")
        .expect("PAGERDUTY_ROUTING_KEY environment variable required");

    // Choose your configuration:
    let config = house_of_stake_config(&routing_key);

    // Or monitor a custom contract:
    // let config = contract_events_config(&routing_key, "your-contract.near", Some("nep141"));

    // Or monitor transactions:
    // let config = transaction_monitor_config(&routing_key, "your-contract.near");

    log::info!(
        "Starting NEAR event monitor with {} subscription(s)",
        config.subscriptions.len()
    );

    let monitor = near_pagerduty_alerts::NearPagerDutyMonitor::new(config);
    monitor.start().await?;

    Ok(())
}

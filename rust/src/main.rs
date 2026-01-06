//! Main entry point for the NEAR PagerDuty Monitor binary

use near_pagerduty_alerts::venear_pause_config;
use near_pagerduty_alerts::PagerDutyAlertConfig;
use std::path::Path;
// Uncomment as needed:
// use near_pagerduty_alerts::method_call_config;

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
        log::info!("No config.yaml found, using hardcoded veNEAR pause monitor configuration");
        let routing_key = std::env::var("PAGERDUTY_ROUTING_KEY").expect(
            "PAGERDUTY_ROUTING_KEY environment variable required when no config.yaml is present",
        );

        let venear_contract = std::env::var("VENEAR_CONTRACT")
            .unwrap_or_else(|_| "venear.near".to_string());

        // Monitor veNEAR pause calls
        venear_pause_config(&routing_key, &venear_contract)

        // Or monitor any contract method:
        // method_call_config(&routing_key, "your-contract.near", Some("your_method"))
    };

    log::info!(
        "Starting NEAR action monitor with {} subscription(s)",
        config.subscriptions.len()
    );

    for sub in &config.subscriptions {
        log::info!(
            "  - {}: account={}, method={:?}",
            sub.name,
            sub.account_id,
            sub.method_name
        );
    }

    let monitor = near_pagerduty_alerts::NearPagerDutyMonitor::new(config);
    monitor.start().await?;

    Ok(())
}

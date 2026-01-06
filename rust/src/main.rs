//! Main entry point for the NEAR PagerDuty Monitor binary

use axum::{routing::get, Router};
use near_pagerduty_alerts::venear_pause_config;
use near_pagerduty_alerts::PagerDutyAlertConfig;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::Path;

fn load_config_from_file(path: &str) -> Result<PagerDutyAlertConfig, anyhow::Error> {
    let content = std::fs::read_to_string(path)?;
    let mut config: PagerDutyAlertConfig = serde_yaml::from_str(&content)?;

    // If routing key is not in config file, get it from environment variable
    if config.routing_key.is_empty() {
        config.routing_key = std::env::var("PAGERDUTY_ROUTING_KEY").map_err(|_| {
            anyhow::anyhow!(
                "PAGERDUTY_ROUTING_KEY must be set either in config.yaml or as an environment variable"
            )
        })?;
        log::info!("Using PAGERDUTY_ROUTING_KEY from environment variable");
    }

    Ok(config)
}

/// Health check endpoint
async fn health() -> &'static str {
    "OK"
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

        let venear_contract =
            std::env::var("VENEAR_CONTRACT").unwrap_or_else(|_| "venear.near".to_string());

        venear_pause_config(&routing_key, &venear_contract)
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

    // Start HTTP server for health checks
    let app = Router::new().route("/health", get(health));

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .unwrap_or(8080);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    log::info!("Starting HTTP server on {}", addr);

    // Run HTTP server and monitor concurrently
    let monitor = near_pagerduty_alerts::NearPagerDutyMonitor::new(config);

    tokio::select! {
        result = axum::serve(tokio::net::TcpListener::bind(addr).await?, app).into_future() => {
            log::error!("HTTP server exited: {:?}", result);
        }
        result = monitor.start() => {
            log::error!("Monitor exited: {:?}", result);
        }
    }

    Ok(())
}

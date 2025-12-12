//! NEAR Blockchain Event to PagerDuty Alert Bridge
//!
//! This module provides a PagerDuty alerting system that mirrors the architecture
//! of the Tear bot's House of Stake module, but sends alerts to PagerDuty instead
//! of Telegram.
//!
//! # Architecture
//!
//! The system connects to Intear's WebSocket Events API (same as Tear bot) and
//! triggers PagerDuty alerts when matching events are detected.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};

// =============================================================================
// Configuration Types
// =============================================================================

/// Configuration for the PagerDuty alerting system
#[derive(Debug, Clone, Deserialize)]
pub struct PagerDutyAlertConfig {
    /// PagerDuty integration/routing key
    pub routing_key: String,
    /// List of event subscriptions to monitor
    pub subscriptions: Vec<EventSubscription>,
    /// Reconnection delay in seconds (default: 5)
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_secs: u64,
}

fn default_reconnect_delay() -> u64 {
    5
}

/// A single event subscription that triggers PagerDuty alerts
#[derive(Debug, Clone, Deserialize)]
pub struct EventSubscription {
    /// Human-readable name for this subscription
    pub name: String,
    /// Intear event type (e.g., "log_nep297", "ft_transfer", "tx_transaction")
    pub event_type: String,
    /// Intear filter object (JSON)
    pub filter: serde_json::Value,
    /// PagerDuty severity: critical, error, warning, info
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Summary template (can include placeholders like {account_id})
    #[serde(default)]
    pub summary_template: Option<String>,
    /// Whether to use testnet
    #[serde(default)]
    pub testnet: bool,
    /// Optional dedup key template
    #[serde(default)]
    pub dedup_key_template: Option<String>,
}

fn default_severity() -> String {
    "warning".to_string()
}

// =============================================================================
// PagerDuty Client
// =============================================================================

/// PagerDuty Events API v2 client
pub struct PagerDutyClient {
    client: reqwest::Client,
    routing_key: String,
}

#[derive(Debug, Serialize)]
struct PagerDutyEvent {
    routing_key: String,
    event_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dedup_key: Option<String>,
    payload: PagerDutyPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<Vec<PagerDutyLink>>,
    client: String,
    client_url: String,
}

#[derive(Debug, Serialize)]
struct PagerDutyPayload {
    summary: String,
    source: String,
    severity: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct PagerDutyLink {
    href: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct PagerDutyResponse {
    status: String,
    message: String,
    dedup_key: Option<String>,
}

impl PagerDutyClient {
    const EVENTS_URL: &'static str = "https://events.pagerduty.com/v2/enqueue";

    pub fn new(routing_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            routing_key,
        }
    }

    /// Trigger a PagerDuty alert
    pub async fn trigger(
        &self,
        summary: &str,
        source: &str,
        severity: &str,
        dedup_key: Option<String>,
        custom_details: Option<serde_json::Value>,
        explorer_link: Option<(&str, &str)>,
    ) -> Result<PagerDutyResponse, anyhow::Error> {
        let links = explorer_link.map(|(href, text)| {
            vec![PagerDutyLink {
                href: href.to_string(),
                text: text.to_string(),
            }]
        });

        let event = PagerDutyEvent {
            routing_key: self.routing_key.clone(),
            event_action: "trigger".to_string(),
            dedup_key,
            payload: PagerDutyPayload {
                summary: summary.chars().take(1024).collect(), // PD limit
                source: source.to_string(),
                severity: severity.to_string(),
                timestamp: Utc::now().to_rfc3339(),
                custom_details,
            },
            links,
            client: "NEAR Blockchain Monitor".to_string(),
            client_url: "https://explorer.near.org".to_string(),
        };

        let response = self
            .client
            .post(Self::EVENTS_URL)
            .json(&event)
            .send()
            .await?;

        let result: PagerDutyResponse = response.json().await?;
        log::info!("PagerDuty response: {:?}", result);
        Ok(result)
    }

    /// Acknowledge an existing alert
    pub async fn acknowledge(&self, dedup_key: &str) -> Result<PagerDutyResponse, anyhow::Error> {
        let event = serde_json::json!({
            "routing_key": self.routing_key,
            "event_action": "acknowledge",
            "dedup_key": dedup_key,
        });

        let response = self
            .client
            .post(Self::EVENTS_URL)
            .json(&event)
            .send()
            .await?;

        Ok(response.json().await?)
    }

    /// Resolve an existing alert
    pub async fn resolve(&self, dedup_key: &str) -> Result<PagerDutyResponse, anyhow::Error> {
        let event = serde_json::json!({
            "routing_key": self.routing_key,
            "event_action": "resolve",
            "dedup_key": dedup_key,
        });

        let response = self
            .client
            .post(Self::EVENTS_URL)
            .json(&event)
            .send()
            .await?;

        Ok(response.json().await?)
    }
}

// =============================================================================
// Event Monitor
// =============================================================================

/// Main event monitoring service
pub struct NearPagerDutyMonitor {
    config: PagerDutyAlertConfig,
    pd_client: Arc<PagerDutyClient>,
}

impl NearPagerDutyMonitor {
    pub fn new(config: PagerDutyAlertConfig) -> Self {
        let pd_client = Arc::new(PagerDutyClient::new(config.routing_key.clone()));
        Self { config, pd_client }
    }

    /// Start monitoring all configured event streams
    pub async fn start(&self) -> Result<(), anyhow::Error> {
        let mut handles = Vec::new();

        for subscription in &self.config.subscriptions {
            let pd_client = Arc::clone(&self.pd_client);
            let subscription = subscription.clone();
            let reconnect_delay = self.config.reconnect_delay_secs;

            let handle = tokio::spawn(async move {
                loop {
                    if let Err(e) =
                        Self::monitor_stream(&subscription, &pd_client).await
                    {
                        log::error!(
                            "Error in subscription '{}': {:?}",
                            subscription.name,
                            e
                        );
                    }
                    log::info!(
                        "Reconnecting to '{}' in {}s...",
                        subscription.name,
                        reconnect_delay
                    );
                    tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
                }
            });

            handles.push(handle);
        }

        // Wait for all handles (they run forever unless errored)
        for handle in handles {
            let _ = handle.await;
        }

        Ok(())
    }

    /// Monitor a single event stream
    async fn monitor_stream(
        subscription: &EventSubscription,
        pd_client: &PagerDutyClient,
    ) -> Result<(), anyhow::Error> {
        let ws_url = Self::get_ws_url(&subscription.event_type, subscription.testnet);
        log::info!("Connecting to {} for '{}'", ws_url, subscription.name);

        let (mut ws_stream, _) = connect_async(&ws_url).await?;

        // Send filter
        let filter_json = serde_json::to_string(&subscription.filter)?;
        ws_stream.send(Message::Text(filter_json)).await?;
        log::info!(
            "Connected and filter sent for '{}': {}",
            subscription.name,
            subscription.filter
        );

        while let Some(msg) = ws_stream.next().await {
            match msg? {
                Message::Text(text) => {
                    // Events come as an array (grouped by block)
                    let events: Vec<serde_json::Value> = serde_json::from_str(&text)?;
                    for event in events {
                        Self::process_event(&event, subscription, pd_client).await?;
                    }
                }
                Message::Ping(data) => {
                    ws_stream.send(Message::Pong(data)).await?;
                }
                Message::Close(_) => {
                    log::warn!("WebSocket closed for '{}'", subscription.name);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn get_ws_url(event_type: &str, testnet: bool) -> String {
        let base = if testnet {
            "wss://ws-events-v3-testnet.intear.tech/events"
        } else {
            "wss://ws-events-v3.intear.tech/events"
        };
        format!("{}/{}", base, event_type)
    }

    /// Process a single event and send PagerDuty alert
    async fn process_event(
        event: &serde_json::Value,
        subscription: &EventSubscription,
        pd_client: &PagerDutyClient,
    ) -> Result<(), anyhow::Error> {
        let account_id = event
            .get("account_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        log::info!(
            "Event received for '{}': {}",
            subscription.name,
            account_id
        );

        // Format summary
        let summary = Self::format_summary(event, subscription);

        // Generate dedup key
        let dedup_key = Self::format_dedup_key(event, subscription);

        // Get explorer link
        let explorer_link = Self::get_explorer_link(event, subscription.testnet);

        // Create custom details
        let custom_details = serde_json::json!({
            "subscription_name": subscription.name,
            "event_type": subscription.event_type,
            "raw_event": event,
        });

        pd_client
            .trigger(
                &summary,
                &format!("near:{}", account_id),
                &subscription.severity,
                dedup_key,
                Some(custom_details),
                explorer_link.as_ref().map(|(h, t)| (h.as_str(), t.as_str())),
            )
            .await?;

        Ok(())
    }

    fn format_summary(event: &serde_json::Value, subscription: &EventSubscription) -> String {
        if let Some(template) = &subscription.summary_template {
            // Simple template replacement
            let mut result = template.clone();
            if let Some(obj) = event.as_object() {
                for (key, value) in obj {
                    let placeholder = format!("{{{}}}", key);
                    let value_str = match value {
                        serde_json::Value::String(s) => s.clone(),
                        _ => value.to_string(),
                    };
                    result = result.replace(&placeholder, &value_str);
                }
            }
            result
        } else {
            let account_id = event
                .get("account_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("{}: Event from {}", subscription.name, account_id)
        }
    }

    fn format_dedup_key(
        event: &serde_json::Value,
        subscription: &EventSubscription,
    ) -> Option<String> {
        if let Some(template) = &subscription.dedup_key_template {
            let mut result = template.clone();
            if let Some(obj) = event.as_object() {
                for (key, value) in obj {
                    let placeholder = format!("{{{}}}", key);
                    let value_str = match value {
                        serde_json::Value::String(s) => s.clone(),
                        _ => value.to_string(),
                    };
                    result = result.replace(&placeholder, &value_str);
                }
            }
            Some(result)
        } else {
            // Default to transaction_id or receipt_id
            event
                .get("transaction_id")
                .or_else(|| event.get("receipt_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }
    }

    fn get_explorer_link(event: &serde_json::Value, testnet: bool) -> Option<(String, String)> {
        let base = if testnet {
            "https://testnet.nearblocks.io"
        } else {
            "https://nearblocks.io"
        };

        if let Some(tx_id) = event.get("transaction_id").and_then(|v| v.as_str()) {
            return Some((format!("{}/txns/{}", base, tx_id), "View Transaction".to_string()));
        }

        if let Some(account_id) = event.get("account_id").and_then(|v| v.as_str()) {
            return Some((
                format!("{}/address/{}", base, account_id),
                "View Contract".to_string(),
            ));
        }

        None
    }
}

// =============================================================================
// Example Configurations
// =============================================================================

/// Create House of Stake monitoring config (mirrors the Telegram bot)
pub fn house_of_stake_config(routing_key: &str) -> PagerDutyAlertConfig {
    PagerDutyAlertConfig {
        routing_key: routing_key.to_string(),
        reconnect_delay_secs: 5,
        subscriptions: vec![
            EventSubscription {
                name: "HoS: New Proposal".to_string(),
                event_type: "log_nep297".to_string(),
                filter: serde_json::json!({
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "create_proposal"}},
                    ]
                }),
                severity: "warning".to_string(),
                summary_template: Some("House of Stake: New proposal created".to_string()),
                testnet: false,
                dedup_key_template: Some("hos-proposal-{transaction_id}".to_string()),
            },
            EventSubscription {
                name: "HoS: Proposal Approved".to_string(),
                event_type: "log_nep297".to_string(),
                filter: serde_json::json!({
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "proposal_approve"}},
                    ]
                }),
                severity: "info".to_string(),
                summary_template: Some("House of Stake: Proposal approved for voting".to_string()),
                testnet: false,
                dedup_key_template: Some("hos-approve-{transaction_id}".to_string()),
            },
            EventSubscription {
                name: "HoS: Vote Cast".to_string(),
                event_type: "log_nep297".to_string(),
                filter: serde_json::json!({
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "add_vote"}},
                    ]
                }),
                severity: "info".to_string(),
                summary_template: Some("House of Stake: Vote cast on proposal".to_string()),
                testnet: false,
                dedup_key_template: Some("hos-vote-{transaction_id}".to_string()),
            },
        ],
    }
}

/// Create config for monitoring any contract's NEP-297 events
pub fn contract_events_config(
    routing_key: &str,
    contract_id: &str,
    event_standard: Option<&str>,
) -> PagerDutyAlertConfig {
    let mut filter_conditions = vec![serde_json::json!({
        "path": "account_id",
        "operator": {"Equals": contract_id}
    })];

    if let Some(standard) = event_standard {
        filter_conditions.push(serde_json::json!({
            "path": "event_standard",
            "operator": {"Equals": standard}
        }));
    }

    PagerDutyAlertConfig {
        routing_key: routing_key.to_string(),
        reconnect_delay_secs: 5,
        subscriptions: vec![EventSubscription {
            name: format!("Contract Events: {}", contract_id),
            event_type: "log_nep297".to_string(),
            filter: serde_json::json!({"And": filter_conditions}),
            severity: "warning".to_string(),
            summary_template: Some(format!(
                "Event on {}: {{event_event}}",
                contract_id
            )),
            testnet: false,
            dedup_key_template: Some(format!("{}-{{transaction_id}}", contract_id)),
        }],
    }
}

/// Create config for monitoring transactions to a specific contract
pub fn transaction_monitor_config(routing_key: &str, contract_id: &str) -> PagerDutyAlertConfig {
    PagerDutyAlertConfig {
        routing_key: routing_key.to_string(),
        reconnect_delay_secs: 5,
        subscriptions: vec![EventSubscription {
            name: format!("Transactions to: {}", contract_id),
            event_type: "tx_transaction".to_string(),
            filter: serde_json::json!({
                "And": [
                    {"path": "receiver_id", "operator": {"Equals": contract_id}}
                ]
            }),
            severity: "warning".to_string(),
            summary_template: Some(format!(
                "Transaction to {} from {{signer_id}}",
                contract_id
            )),
            testnet: false,
            dedup_key_template: Some(format!("tx-{}-{{transaction_id}}", contract_id)),
        }],
    }
}

// =============================================================================
// Main Entry Point Example
// =============================================================================

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

    let monitor = NearPagerDutyMonitor::new(config);
    monitor.start().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_house_of_stake_config() {
        let config = house_of_stake_config("test-key");
        assert_eq!(config.subscriptions.len(), 3);
        assert_eq!(config.subscriptions[0].name, "HoS: New Proposal");
    }

    #[test]
    fn test_contract_events_config() {
        let config = contract_events_config("test-key", "test.near", Some("nep141"));
        assert_eq!(config.subscriptions.len(), 1);
    }
}

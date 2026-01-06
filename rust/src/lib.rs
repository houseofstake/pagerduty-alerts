//! NEAR Blockchain Action Monitor to PagerDuty Alert Bridge
//!
//! This module monitors NEAR blockchain actions via the neardata WebSocket stream
//! and triggers PagerDuty alerts when matching events are detected.
//!
//! # Architecture
//!
//! The system connects to neardata's WebSocket API (wss://actions.near.stream/ws)
//! and filters for specific contract calls, optionally filtering by method name.

use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};

// =============================================================================
// Configuration Types
// =============================================================================

/// Configuration for the PagerDuty alerting system
#[derive(Debug, Clone, Deserialize)]
pub struct PagerDutyAlertConfig {
    /// PagerDuty integration/routing key (can be omitted from YAML to use env var)
    #[serde(rename = "pagerduty_routing_key", default = "default_routing_key")]
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

fn default_routing_key() -> String {
    String::new()
}

/// A single event subscription that triggers PagerDuty alerts
#[derive(Debug, Clone, Deserialize)]
pub struct EventSubscription {
    /// Human-readable name for this subscription
    pub name: String,
    /// The contract account ID to monitor
    pub account_id: String,
    /// Optional method name filter - if set, only alerts for this method
    #[serde(default)]
    pub method_name: Option<String>,
    /// PagerDuty severity: critical, error, warning, info
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Summary template (can include placeholders like {account_id}, {method_name}, {predecessor_id})
    #[serde(default)]
    pub summary_template: Option<String>,
    /// Optional dedup key template
    #[serde(default)]
    pub dedup_key_template: Option<String>,
}

fn default_severity() -> String {
    "warning".to_string()
}

// =============================================================================
// Neardata Types
// =============================================================================

/// Message received from neardata WebSocket
#[derive(Debug, Deserialize)]
struct NeardataMessage {
    #[allow(dead_code)]
    secret: String,
    actions: Vec<NeardataAction>,
    #[allow(dead_code)]
    note: Option<String>,
}

/// A single action from neardata
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeardataAction {
    pub block_height: u64,
    #[serde(default)]
    pub block_hash: Option<String>,
    #[serde(default)]
    pub block_timestamp_ms: Option<f64>,
    #[serde(default)]
    pub tx_hash: Option<String>,
    #[serde(default)]
    pub receipt_id: Option<String>,
    #[serde(default)]
    pub signer_id: Option<String>,
    pub account_id: String,
    #[serde(default)]
    pub predecessor_id: Option<String>,
    pub status: String,
    pub action: ActionType,
}

/// The type of action
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ActionType {
    FunctionCall(FunctionCallAction),
    Transfer(TransferAction),
    DeployContract(DeployContractAction),
    AddKey(AddKeyAction),
    DeleteKey(DeleteKeyAction),
    CreateAccount(CreateAccountAction),
    DeleteAccount(DeleteAccountAction),
    Stake(StakeAction),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCallAction {
    pub method_name: String,
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub deposit: Option<String>,
    #[serde(default)]
    pub gas: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransferAction {
    pub deposit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeployContractAction {
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AddKeyAction {
    pub public_key: String,
    #[serde(default)]
    pub access_key: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeleteKeyAction {
    pub public_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateAccountAction {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeleteAccountAction {
    #[serde(default)]
    pub beneficiary_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StakeAction {
    pub stake: String,
    pub public_key: String,
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
pub struct PagerDutyResponse {
    pub status: String,
    pub message: String,
    pub dedup_key: Option<String>,
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
            client_url: "https://nearblocks.io".to_string(),
        };

        let response = self
            .client
            .post(Self::EVENTS_URL)
            .json(&event)
            .send()
            .await?;

        let result: PagerDutyResponse = response.json().await?;
        log::info!(
            "PagerDuty alert triggered: status={}, message={}, dedup_key={:?}",
            result.status,
            result.message,
            result.dedup_key
        );
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

        let result: PagerDutyResponse = response.json().await?;
        log::info!(
            "PagerDuty alert acknowledged: status={}, message={}",
            result.status,
            result.message
        );
        Ok(result)
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

        let result: PagerDutyResponse = response.json().await?;
        log::info!(
            "PagerDuty alert resolved: status={}, message={}",
            result.status,
            result.message
        );
        Ok(result)
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
    const NEARDATA_WS_URL: &'static str = "wss://actions.near.stream/ws";

    pub fn new(config: PagerDutyAlertConfig) -> Self {
        let pd_client = Arc::new(PagerDutyClient::new(config.routing_key.clone()));
        Self { config, pd_client }
    }

    /// Start monitoring - connects to neardata and processes actions
    pub async fn start(&self) -> Result<(), anyhow::Error> {
        loop {
            if let Err(e) = self.monitor_stream().await {
                log::error!("Error in neardata stream: {:?}", e);
            }
            log::info!(
                "Reconnecting to neardata in {}s...",
                self.config.reconnect_delay_secs
            );
            tokio::time::sleep(Duration::from_secs(self.config.reconnect_delay_secs)).await;
        }
    }

    /// Monitor the neardata WebSocket stream
    async fn monitor_stream(&self) -> Result<(), anyhow::Error> {
        log::info!("Connecting to {}", Self::NEARDATA_WS_URL);

        let (mut ws_stream, _) = connect_async(Self::NEARDATA_WS_URL).await?;

        // Build filter for all monitored accounts
        let account_ids: Vec<&str> = self
            .config
            .subscriptions
            .iter()
            .map(|s| s.account_id.as_str())
            .collect();

        // Build subscription lookup by account_id for fast matching
        let subscriptions_by_account: HashMap<&str, Vec<&EventSubscription>> = {
            let mut map: HashMap<&str, Vec<&EventSubscription>> = HashMap::new();
            for sub in &self.config.subscriptions {
                map.entry(sub.account_id.as_str())
                    .or_default()
                    .push(sub);
            }
            map
        };

        // Neardata filter format
        let filter = serde_json::json!({
            "secret": "tmp",
            "filter": account_ids.iter().map(|id| {
                serde_json::json!({"accountId": id, "status": "SUCCESS"})
            }).collect::<Vec<_>>(),
            "fetch_past_actions": 0
        });

        let filter_json = serde_json::to_string(&filter)?;
        ws_stream.send(Message::Text(filter_json.clone())).await?;
        log::info!("Connected and filter sent: {}", filter_json);

        while let Some(msg) = ws_stream.next().await {
            match msg? {
                Message::Text(text) => {
                    match serde_json::from_str::<NeardataMessage>(&text) {
                        Ok(neardata_msg) => {
                            for action in neardata_msg.actions {
                                // Find matching subscriptions for this account
                                if let Some(subs) = subscriptions_by_account.get(action.account_id.as_str()) {
                                    for sub in subs {
                                        if Self::action_matches_subscription(&action, sub) {
                                            if let Err(e) = self.process_action(&action, sub).await {
                                                log::error!("Error processing action: {:?}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to parse neardata message: {:?}", e);
                            log::debug!("Raw message: {}", text);
                        }
                    }
                }
                Message::Ping(data) => {
                    ws_stream.send(Message::Pong(data)).await?;
                }
                Message::Close(_) => {
                    log::warn!("WebSocket closed");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Check if an action matches a subscription's filters
    fn action_matches_subscription(action: &NeardataAction, subscription: &EventSubscription) -> bool {
        // If method_name filter is set, only match FunctionCall with that method
        if let Some(ref required_method) = subscription.method_name {
            match &action.action {
                ActionType::FunctionCall(fc) => {
                    if fc.method_name != *required_method {
                        return false;
                    }
                }
                _ => return false, // Not a function call, doesn't match
            }
        }
        true
    }

    /// Process an action and send PagerDuty alert
    async fn process_action(
        &self,
        action: &NeardataAction,
        subscription: &EventSubscription,
    ) -> Result<(), anyhow::Error> {
        let method_name = match &action.action {
            ActionType::FunctionCall(fc) => Some(fc.method_name.as_str()),
            _ => None,
        };

        log::info!(
            "Action matched for '{}': account={}, method={:?}, from={:?}",
            subscription.name,
            action.account_id,
            method_name,
            action.predecessor_id
        );

        // Format summary
        let summary = self.format_summary(action, subscription);

        // Generate dedup key
        let dedup_key = self.format_dedup_key(action, subscription);

        // Get explorer link
        let explorer_link = Self::get_explorer_link(action);

        // Create custom details
        let custom_details = serde_json::json!({
            "subscription_name": subscription.name,
            "account_id": action.account_id,
            "method_name": method_name,
            "predecessor_id": action.predecessor_id,
            "signer_id": action.signer_id,
            "block_height": action.block_height,
            "tx_hash": action.tx_hash,
            "receipt_id": action.receipt_id,
            "action": action.action,
        });

        self.pd_client
            .trigger(
                &summary,
                &format!("near:{}", action.account_id),
                &subscription.severity,
                dedup_key,
                Some(custom_details),
                explorer_link
                    .as_ref()
                    .map(|(h, t)| (h.as_str(), t.as_str())),
            )
            .await?;

        Ok(())
    }

    fn format_summary(&self, action: &NeardataAction, subscription: &EventSubscription) -> String {
        if let Some(template) = &subscription.summary_template {
            let method_name = match &action.action {
                ActionType::FunctionCall(fc) => fc.method_name.clone(),
                _ => "unknown".to_string(),
            };

            template
                .replace("{account_id}", &action.account_id)
                .replace("{method_name}", &method_name)
                .replace("{predecessor_id}", action.predecessor_id.as_deref().unwrap_or("unknown"))
                .replace("{signer_id}", action.signer_id.as_deref().unwrap_or("unknown"))
                .replace("{block_height}", &action.block_height.to_string())
                .replace("{tx_hash}", action.tx_hash.as_deref().unwrap_or("unknown"))
        } else {
            let method_name = match &action.action {
                ActionType::FunctionCall(fc) => format!(" calling {}", fc.method_name),
                _ => String::new(),
            };
            format!(
                "{}: Action on {}{}",
                subscription.name, action.account_id, method_name
            )
        }
    }

    fn format_dedup_key(
        &self,
        action: &NeardataAction,
        subscription: &EventSubscription,
    ) -> Option<String> {
        if let Some(template) = &subscription.dedup_key_template {
            let method_name = match &action.action {
                ActionType::FunctionCall(fc) => fc.method_name.clone(),
                _ => "unknown".to_string(),
            };

            Some(
                template
                    .replace("{account_id}", &action.account_id)
                    .replace("{method_name}", &method_name)
                    .replace("{predecessor_id}", action.predecessor_id.as_deref().unwrap_or("unknown"))
                    .replace("{signer_id}", action.signer_id.as_deref().unwrap_or("unknown"))
                    .replace("{block_height}", &action.block_height.to_string())
                    .replace("{tx_hash}", action.tx_hash.as_deref().unwrap_or("unknown"))
                    .replace("{receipt_id}", action.receipt_id.as_deref().unwrap_or("unknown")),
            )
        } else {
            // Default to tx_hash or receipt_id
            action
                .tx_hash
                .clone()
                .or_else(|| action.receipt_id.clone())
        }
    }

    fn get_explorer_link(action: &NeardataAction) -> Option<(String, String)> {
        if let Some(ref tx_hash) = action.tx_hash {
            return Some((
                format!("https://nearblocks.io/txns/{}", tx_hash),
                "View Transaction".to_string(),
            ));
        }

        Some((
            format!("https://nearblocks.io/address/{}", action.account_id),
            "View Contract".to_string(),
        ))
    }
}

// =============================================================================
// Example Configurations
// =============================================================================

/// Create config for monitoring veNEAR pause calls
pub fn venear_pause_config(routing_key: &str, venear_contract: &str) -> PagerDutyAlertConfig {
    PagerDutyAlertConfig {
        routing_key: routing_key.to_string(),
        reconnect_delay_secs: 5,
        subscriptions: vec![EventSubscription {
            name: "veNEAR: Contract Paused".to_string(),
            account_id: venear_contract.to_string(),
            method_name: Some("pause".to_string()),
            severity: "critical".to_string(),
            summary_template: Some(
                "CRITICAL: veNEAR contract paused by {predecessor_id}".to_string(),
            ),
            dedup_key_template: Some("venear-pause-{tx_hash}".to_string()),
        }],
    }
}

/// Create config for monitoring any contract method calls
pub fn method_call_config(
    routing_key: &str,
    contract_id: &str,
    method_name: Option<&str>,
) -> PagerDutyAlertConfig {
    PagerDutyAlertConfig {
        routing_key: routing_key.to_string(),
        reconnect_delay_secs: 5,
        subscriptions: vec![EventSubscription {
            name: format!(
                "Contract Call: {}{}",
                contract_id,
                method_name.map(|m| format!("::{}", m)).unwrap_or_default()
            ),
            account_id: contract_id.to_string(),
            method_name: method_name.map(String::from),
            severity: "warning".to_string(),
            summary_template: Some(format!(
                "Call to {} - {{method_name}} from {{predecessor_id}}",
                contract_id
            )),
            dedup_key_template: Some(format!("{}-{{tx_hash}}", contract_id)),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_venear_pause_config() {
        let config = venear_pause_config("test-key", "venear.near");
        assert_eq!(config.subscriptions.len(), 1);
        assert_eq!(config.subscriptions[0].method_name, Some("pause".to_string()));
    }

    #[test]
    fn test_method_call_config() {
        let config = method_call_config("test-key", "test.near", Some("transfer"));
        assert_eq!(config.subscriptions.len(), 1);
        assert_eq!(
            config.subscriptions[0].method_name,
            Some("transfer".to_string())
        );
    }
}

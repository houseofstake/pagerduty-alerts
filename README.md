# NEAR Blockchain to PagerDuty Alert Bridge

Monitor NEAR blockchain actions and trigger PagerDuty alerts when specific on-chain events occur. Built for monitoring House of Stake (veNEAR) contracts.

## Architecture

```
┌─────────────────────┐                    ┌──────────────────────┐
│   NEAR Blockchain   │ ──────────────────►│  neardata Actions    │
│      (mainnet)      │                    │  Stream API          │
└─────────────────────┘                    └──────────┬───────────┘
                                                      │
                                                      │ WebSocket
                                                      ▼
                                           ┌──────────────────────┐
                                           │  This Alert Bridge   │
                                           │  (Rust)              │
                                           └──────────┬───────────┘
                                                      │
                                                      │ HTTP POST
                                                      ▼
                                           ┌──────────────────────┐
                                           │  PagerDuty Events    │
                                           │  API v2              │
                                           └──────────────────────┘
```

## Quick Start

```bash
# 1. Build
cd rust
cargo build --release

# 2. Set your PagerDuty integration key
export PAGERDUTY_ROUTING_KEY="your-key-here"

# 3. Run
RUST_LOG=info ./target/release/near-pagerduty-monitor
```

## PagerDuty Setup

1. **Create a Service** in PagerDuty (or use an existing one)
2. **Add an Integration**: Go to Service → Integrations → Add Integration
3. **Select "Events API V2"**
4. **Copy the Integration Key** (also called Routing Key)
5. **Set as environment variable**: `export PAGERDUTY_ROUTING_KEY="..."`

## Configuration

Configuration is done via `config.yaml`. Copy the example and customize:

```bash
cp config.example.yaml config.yaml
```

### Example Configuration

```yaml
# pagerduty_routing_key: "..."  # Or use PAGERDUTY_ROUTING_KEY env var
reconnect_delay_secs: 5

subscriptions:
  # Critical: Monitor contract pause
  - name: "veNEAR: Contract Paused"
    account_id: "venear.near"
    method_name: "pause"
    severity: critical
    summary_template: "CRITICAL: veNEAR contract paused by {predecessor_id}"
    dedup_key_template: "venear-pause-{tx_hash}"

  # Info: Monitor new proposals
  - name: "HoS: New Proposal"
    account_id: "vote.dao"
    method_name: "create_proposal"
    severity: info
    summary_template: "House of Stake: New proposal created by {predecessor_id}"
    dedup_key_template: "hos-proposal-{tx_hash}"

  # Monitor ALL actions on a contract (no method filter)
  - name: "All Contract Actions"
    account_id: "my-contract.near"
    # method_name: omitted = alert on any action
    severity: info
```

### Subscription Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable name for the alert |
| `account_id` | Yes | NEAR contract to monitor |
| `method_name` | No | Filter for specific method calls (omit to match all) |
| `severity` | No | `critical`, `error`, `warning`, `info` (default: `warning`) |
| `summary_template` | No | Alert message with placeholders |
| `dedup_key_template` | No | Deduplication key with placeholders |

### Available Placeholders

| Placeholder | Description |
|-------------|-------------|
| `{account_id}` | Contract that received the action |
| `{method_name}` | Method that was called |
| `{predecessor_id}` | Immediate caller of the contract |
| `{signer_id}` | Transaction signer |
| `{tx_hash}` | Transaction hash |
| `{receipt_id}` | Receipt ID |
| `{block_height}` | Block height |

## Severity Levels

| Level | PagerDuty Behavior |
|-------|-------------------|
| `critical` | Pages immediately |
| `error` | High priority |
| `warning` | Medium priority (default) |
| `info` | Low priority / informational |

## Deployment

### Railway

1. Connect your GitHub repository at [railway.app](https://railway.app)
2. Set environment variables:
   - `PAGERDUTY_ROUTING_KEY` - Your PagerDuty integration key
   - `RUST_LOG` - Logging level (optional, default: `info`)
3. Deploy - Railway will use the included `Dockerfile`

### Docker

```bash
docker build -t near-pagerduty-alerts .
docker run -e PAGERDUTY_ROUTING_KEY=your-key near-pagerduty-alerts
```

### Systemd Service

```ini
[Unit]
Description=NEAR PagerDuty Alert Bridge
After=network.target

[Service]
Type=simple
User=near-alerts
Environment=PAGERDUTY_ROUTING_KEY=your-key
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/near-pagerduty-monitor
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

## How It Works

The monitor connects to the [neardata WebSocket stream](https://github.com/evgenykuzyakov/event-api) (`wss://actions.near.stream/ws`) which provides real-time NEAR blockchain actions including:

- Function calls (with method names)
- Transfers
- Contract deployments
- Key management

When an action matches a subscription's filters (account + optional method), a PagerDuty alert is triggered with:
- Configurable severity
- Link to transaction on nearblocks.io
- Full action details in custom fields

## Troubleshooting

### No events received
1. Verify `account_id` is correct (exact match required)
2. Check WebSocket connection in logs
3. Test with no `method_name` filter to see all actions

### PagerDuty alerts not triggering
1. Verify routing key is correct
2. Check PagerDuty service is not in maintenance
3. Look at logs for HTTP response errors

### Connection drops
- Auto-reconnects after `reconnect_delay_secs`
- Check network stability
- Logs will show "Reconnecting to neardata..."

## License

MIT

## Credits

- [neardata Actions Stream](https://github.com/evgenykuzyakov/event-api) - Blockchain action streaming
- [PagerDuty Events API v2](https://developer.pagerduty.com/docs/events-api-v2/overview/) - Alert delivery

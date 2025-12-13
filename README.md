# NEAR Blockchain to PagerDuty Alert Bridge

Monitor NEAR blockchain events and trigger PagerDuty alerts when specific on-chain events occur. This system mirrors the architecture of the [Tear Telegram bot](https://github.com/INTEARnear/Tear)'s House of Stake module, but sends alerts to PagerDuty instead of Telegram.

## Architecture

```
┌─────────────────────┐     WebSocket      ┌──────────────────────┐
│   NEAR Blockchain   │ ─────────────────► │  Intear Events API   │
│   (mainnet/testnet) │                    │  (ws-events-v3)      │
└─────────────────────┘                    └──────────┬───────────┘
                                                      │
                                                      │ Filter & Stream
                                                      ▼
                                           ┌──────────────────────┐
                                           │  This Alert Bridge   │
                                           │  (Python or Rust)    │
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

### Option 1: Python (Fastest Setup)

```bash
# 1. Install dependencies
pip install aiohttp

# 2. Set your PagerDuty integration key
export PAGERDUTY_ROUTING_KEY="your-key-here"

# 3. Run the monitor
python python/near_pagerduty_bridge.py
```

### Option 2: Rust (Production)

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

### Pre-built Configurations

Both implementations include ready-to-use configurations:

#### House of Stake Monitoring
Monitors the same events as the Tear Telegram bot:
- New proposal created
- Proposal approved for voting
- Votes cast

```python
# Python
config = create_house_of_stake_config(routing_key)
```

```rust
// Rust
let config = house_of_stake_config(&routing_key);
```

#### Custom Contract Monitoring
Monitor any contract's NEP-297 events:

```python
# Python
config = create_custom_contract_config(
    routing_key,
    contract_id="your-contract.near",
    event_standard="nep141",  # Optional: filter by standard
)
```

```rust
// Rust
let config = contract_events_config(&routing_key, "your-contract.near", Some("nep141"));
```

#### Transaction Monitoring
Monitor all transactions to a specific contract:

```python
# Python  
config = create_function_call_config(routing_key, "your-contract.near")
```

```rust
// Rust
let config = transaction_monitor_config(&routing_key, "your-contract.near");
```

### YAML Configuration

For more complex setups, use the YAML config file (see `config.example.yaml`):

```yaml
pagerduty_routing_key: "YOUR_KEY"
reconnect_delay_secs: 5

subscriptions:
  - name: "My Alert"
    event_type: log_nep297
    severity: warning
    filter:
      And:
        - path: account_id
          operator:
            Equals: "my-contract.near"
        - path: event_event
          operator:
            Equals: "important_event"
```

## Available Event Types

From the [Intear Events API](https://docs.intear.tech/docs/events-api/):

| Event Type | Description |
|------------|-------------|
| `log_nep297` | NEP-297 events (`EVENT_JSON:{...}`) - most common |
| `log_text` | Raw text log events |
| `tx_transaction` | All transactions |
| `tx_receipt` | All receipts |
| `ft_transfer` | Fungible token transfers |
| `ft_mint` | Fungible token minting |
| `ft_burn` | Fungible token burning |
| `nft_mint` | NFT minting |
| `nft_transfer` | NFT transfers |
| `nft_burn` | NFT burning |
| `trade_swap` | DEX swaps |
| `trade_pool_change` | Liquidity pool changes |
| `price_token` | Token price updates |
| `socialdb_index` | SocialDB events |
| `potlock_donation` | Potlock donations |

## Filter Syntax

Filters use the Intear Events API syntax:

### Basic Filter
```json
{
  "path": "account_id",
  "operator": {"Equals": "vote.dao"}
}
```

### Combined Filters
```json
{
  "And": [
    {"path": "account_id", "operator": {"Equals": "vote.dao"}},
    {"path": "event_standard", "operator": {"Equals": "venear"}},
    {"path": "event_event", "operator": {"Equals": "create_proposal"}}
  ]
}
```

### Available Operators

| Category | Operators |
|----------|-----------|
| Comparison | `Equals`, `NotEqual`, `GreaterThan`, `LessThan`, `GreaterOrEqual`, `LessOrEqual` |
| String | `StartsWith`, `EndsWith`, `Contains` |
| Array/Object | `ArrayContains`, `HasKey` |
| Logical | `And`, `Or` |

### Path Syntax
- Dots for nested fields: `data.user.name`
- Brackets for arrays: `tokens[0]`
- `.` for root when using And/Or

## PagerDuty Alert Features

### Severity Levels
- `critical` - Pages immediately
- `error` - High priority
- `warning` - Medium priority (default)
- `info` - Low priority

### Deduplication
Use `dedup_key_template` to control alert grouping:
```yaml
dedup_key_template: "hos-proposal-{transaction_id}"
```

This ensures multiple events from the same transaction don't create duplicate alerts.

### Custom Details
All alerts include:
- Alert name/subscription
- Event type
- Raw event data
- Link to NEAR Explorer

## Deployment Options

### Railway

Railway can deploy your Rust application using Docker or Nixpacks:

1. **Install Railway CLI** (optional, for local deployment):
   ```bash
   npm i -g @railway/cli
   ```

2. **Deploy via Railway Dashboard**:
   - Go to [railway.app](https://railway.app) and create a new project
   - Connect your GitHub repository (or deploy from CLI)
   - Railway will use the `Dockerfile` (recommended) or `nixpacks.toml` configuration

3. **Set Environment Variables**:
   In Railway dashboard → Variables, add:
   - `PAGERDUTY_ROUTING_KEY` - Your PagerDuty integration key (required)
   - `RUST_LOG` - Logging level (optional, default: `info`)

4. **Deploy**:
   Railway will automatically:
   - Build the Rust project using Docker (or Nixpacks)
   - Run the binary `near-pagerduty-monitor`
   - Restart on failure

The repository includes:
- `Dockerfile` - Docker-based build (recommended, more reliable)
- `nixpacks.toml` - Nixpacks configuration (alternative)
- `railway.json` - Railway deployment settings

### Docker
```dockerfile
FROM python:3.11-slim
WORKDIR /app
COPY python/near_pagerduty_bridge.py .
RUN pip install aiohttp
CMD ["python", "near_pagerduty_bridge.py"]
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

### Kubernetes
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: near-pagerduty-bridge
spec:
  replicas: 1
  template:
    spec:
      containers:
      - name: monitor
        image: your-registry/near-pagerduty-bridge
        env:
        - name: PAGERDUTY_ROUTING_KEY
          valueFrom:
            secretKeyRef:
              name: pagerduty-secret
              key: routing-key
```

## Monitoring the Monitor

The system logs all events and PagerDuty responses. For production, consider:

1. **Health checks**: Add an HTTP endpoint that returns OK if WebSocket is connected
2. **Metrics**: Export Prometheus metrics for events processed/alerts sent
3. **Status page**: Use a service like Uptime Robot to ping a heartbeat endpoint

## Comparison with Tear Bot

| Feature | Tear Bot | This Bridge |
|---------|----------|-------------|
| Event Source | Intear WebSocket API | Same |
| Filter Syntax | Same | Same |
| Output | Telegram messages | PagerDuty alerts |
| Persistence | MongoDB | None needed |
| Configuration | Per-chat settings | Config file/code |

## Troubleshooting

### No events received
1. Check WebSocket URL is reachable
2. Verify filter syntax matches Intear API docs
3. Test with empty filter `{"And": []}` to receive all events

### PagerDuty alerts not triggering
1. Verify routing key is correct
2. Check PagerDuty service is not in maintenance
3. Look at custom_details in PagerDuty for raw event data

### Connection drops
- The system auto-reconnects after `reconnect_delay_secs`
- Check network stability
- Consider running multiple instances behind a load balancer

## License

MIT

## Credits

- [Intear Events API](https://docs.intear.tech/docs/events-api/) - Blockchain event streaming
- [Tear Bot](https://github.com/INTEARnear/Tear) - Original Telegram bot architecture
- [PagerDuty Events API v2](https://developer.pagerduty.com/docs/events-api-v2-overview) - Alert delivery

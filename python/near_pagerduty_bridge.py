#!/usr/bin/env python3
"""
NEAR Blockchain Event to PagerDuty Alert Bridge

Connects to Intear's WebSocket Events API and triggers PagerDuty alerts
when specified on-chain events occur.

Usage:
    export PAGERDUTY_ROUTING_KEY="your-integration-key-here"
    python near_pagerduty_bridge.py
"""

import asyncio
import json
import os
import logging
from datetime import datetime
from typing import Optional, Dict, Any, List
from dataclasses import dataclass, field
import aiohttp

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Configuration
INTEAR_WS_URL = "wss://ws-events-v3.intear.tech/events"
INTEAR_WS_TESTNET_URL = "wss://ws-events-v3-testnet.intear.tech/events"
PAGERDUTY_EVENTS_URL = "https://events.pagerduty.com/v2/enqueue"


@dataclass
class AlertConfig:
    """Configuration for a specific alert type"""
    name: str
    event_type: str  # e.g., "log_nep297", "ft_transfer", "tx_transaction"
    filter: Dict[str, Any]  # Intear filter object
    severity: str = "warning"  # critical, error, warning, info
    dedup_key_template: Optional[str] = None  # Template for deduplication
    summary_template: str = "NEAR Event: {event_type}"
    testnet: bool = False


@dataclass 
class EventMonitorConfig:
    """Main configuration for the event monitor"""
    pagerduty_routing_key: str
    alerts: List[AlertConfig] = field(default_factory=list)
    reconnect_delay_secs: int = 5
    client_name: str = "NEAR Blockchain Monitor"
    client_url: str = "https://explorer.near.org"


class PagerDutyClient:
    """Client for sending events to PagerDuty"""
    
    def __init__(self, routing_key: str):
        self.routing_key = routing_key
        self.session: Optional[aiohttp.ClientSession] = None
    
    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        return self
    
    async def __aexit__(self, *args):
        if self.session:
            await self.session.close()
    
    async def trigger(
        self,
        summary: str,
        source: str,
        severity: str = "warning",
        dedup_key: Optional[str] = None,
        custom_details: Optional[Dict[str, Any]] = None,
        links: Optional[List[Dict[str, str]]] = None,
    ) -> Dict[str, Any]:
        """Trigger a PagerDuty alert"""
        payload = {
            "routing_key": self.routing_key,
            "event_action": "trigger",
            "payload": {
                "summary": summary[:1024],  # PD limit
                "source": source,
                "severity": severity,
                "timestamp": datetime.utcnow().isoformat() + "Z",
            },
            "client": "NEAR Blockchain Monitor",
            "client_url": "https://explorer.near.org",
        }
        
        if dedup_key:
            payload["dedup_key"] = dedup_key
        
        if custom_details:
            payload["payload"]["custom_details"] = custom_details
        
        if links:
            payload["links"] = links
        
        async with self.session.post(
            PAGERDUTY_EVENTS_URL,
            json=payload,
            headers={"Content-Type": "application/json"}
        ) as resp:
            result = await resp.json()
            if resp.status == 202:
                logger.info(f"PagerDuty alert triggered: {result.get('dedup_key', 'unknown')}")
            else:
                logger.error(f"PagerDuty error: {result}")
            return result


class IntearEventStream:
    """WebSocket client for Intear Events API"""
    
    def __init__(self, event_type: str, filter_obj: Dict[str, Any], testnet: bool = False):
        self.event_type = event_type
        self.filter_obj = filter_obj
        self.testnet = testnet
        self.ws: Optional[aiohttp.ClientWebSocketResponse] = None
        self.session: Optional[aiohttp.ClientSession] = None
    
    @property
    def ws_url(self) -> str:
        base = INTEAR_WS_TESTNET_URL if self.testnet else INTEAR_WS_URL
        return f"{base}/{self.event_type}"
    
    async def connect(self):
        """Connect to the WebSocket and send filter"""
        self.session = aiohttp.ClientSession()
        self.ws = await self.session.ws_connect(self.ws_url)
        
        # Send filter
        await self.ws.send_str(json.dumps(self.filter_obj))
        logger.info(f"Connected to {self.ws_url} with filter: {self.filter_obj}")
    
    async def close(self):
        if self.ws:
            await self.ws.close()
        if self.session:
            await self.session.close()
    
    def __aiter__(self):
        return self
    
    async def __anext__(self) -> List[Dict[str, Any]]:
        """Receive next batch of events"""
        if not self.ws:
            raise StopAsyncIteration
        
        msg = await self.ws.receive()
        
        if msg.type == aiohttp.WSMsgType.TEXT:
            # Events are grouped by block and sent as an array
            return json.loads(msg.data)
        elif msg.type == aiohttp.WSMsgType.PING:
            await self.ws.pong(msg.data)
            return []
        elif msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.ERROR):
            raise ConnectionError(f"WebSocket closed: {msg.type}")
        
        return []


def format_event_summary(event: Dict[str, Any], template: str, alert_name: str) -> str:
    """Format event summary for PagerDuty"""
    # Extract common fields
    account_id = event.get("account_id", "unknown")
    
    # For NEP-297 events
    event_standard = event.get("event_standard", "")
    event_event = event.get("event_event", "")
    
    # Try to format with available data
    try:
        return template.format(
            alert_name=alert_name,
            account_id=account_id,
            event_standard=event_standard,
            event_event=event_event,
            **event
        )
    except KeyError:
        return f"{alert_name}: Event from {account_id}"


def get_dedup_key(event: Dict[str, Any], template: Optional[str]) -> Optional[str]:
    """Generate deduplication key for event"""
    if not template:
        return None
    
    try:
        return template.format(**event)
    except KeyError:
        # Use transaction/receipt hash if available
        return event.get("transaction_id") or event.get("receipt_id")


def get_explorer_link(event: Dict[str, Any], testnet: bool = False) -> Optional[Dict[str, str]]:
    """Generate NEAR Explorer link for the event"""
    base_url = "https://testnet.nearblocks.io" if testnet else "https://nearblocks.io"
    
    if tx_id := event.get("transaction_id"):
        return {"href": f"{base_url}/txns/{tx_id}", "text": "View Transaction"}
    
    if account_id := event.get("account_id"):
        return {"href": f"{base_url}/address/{account_id}", "text": "View Contract"}
    
    return None


async def monitor_events(config: EventMonitorConfig):
    """Main event monitoring loop"""
    async with PagerDutyClient(config.pagerduty_routing_key) as pd_client:
        tasks = []
        
        for alert in config.alerts:
            task = asyncio.create_task(
                monitor_single_stream(alert, pd_client, config.reconnect_delay_secs)
            )
            tasks.append(task)
        
        await asyncio.gather(*tasks)


async def monitor_single_stream(
    alert: AlertConfig,
    pd_client: PagerDutyClient,
    reconnect_delay: int
):
    """Monitor a single event stream"""
    while True:
        try:
            stream = IntearEventStream(
                alert.event_type,
                alert.filter,
                testnet=alert.testnet
            )
            await stream.connect()
            
            async for events in stream:
                for event in events:
                    await process_event(event, alert, pd_client)
        
        except ConnectionError as e:
            logger.warning(f"Connection lost for {alert.name}: {e}")
        except Exception as e:
            logger.error(f"Error in {alert.name}: {e}", exc_info=True)
        finally:
            try:
                await stream.close()
            except:
                pass
        
        logger.info(f"Reconnecting to {alert.name} in {reconnect_delay}s...")
        await asyncio.sleep(reconnect_delay)


async def process_event(
    event: Dict[str, Any],
    alert: AlertConfig,
    pd_client: PagerDutyClient
):
    """Process a single event and send to PagerDuty"""
    logger.info(f"Event received for {alert.name}: {event.get('account_id', 'unknown')}")
    
    summary = format_event_summary(event, alert.summary_template, alert.name)
    dedup_key = get_dedup_key(event, alert.dedup_key_template)
    link = get_explorer_link(event, alert.testnet)
    
    await pd_client.trigger(
        summary=summary,
        source=f"near:{event.get('account_id', 'unknown')}",
        severity=alert.severity,
        dedup_key=dedup_key,
        custom_details={
            "alert_name": alert.name,
            "event_type": alert.event_type,
            "raw_event": event,
        },
        links=[link] if link else None,
    )


# =============================================================================
# Example Configurations
# =============================================================================

def create_house_of_stake_config(routing_key: str) -> EventMonitorConfig:
    """
    Example: Monitor House of Stake governance events
    Similar to the Telegram bot's house-of-stake module
    """
    return EventMonitorConfig(
        pagerduty_routing_key=routing_key,
        alerts=[
            # New proposal created
            AlertConfig(
                name="HoS: New Proposal",
                event_type="log_nep297",
                filter={
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "create_proposal"}},
                    ]
                },
                severity="warning",
                summary_template="House of Stake: New proposal created by {account_id}",
                dedup_key_template="hos-proposal-{transaction_id}",
            ),
            # Proposal approved
            AlertConfig(
                name="HoS: Proposal Approved",
                event_type="log_nep297",
                filter={
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "proposal_approve"}},
                    ]
                },
                severity="info",
                summary_template="House of Stake: Proposal approved",
                dedup_key_template="hos-approve-{transaction_id}",
            ),
            # Vote cast
            AlertConfig(
                name="HoS: Vote Cast",
                event_type="log_nep297",
                filter={
                    "And": [
                        {"path": "account_id", "operator": {"Equals": "vote.dao"}},
                        {"path": "event_standard", "operator": {"Equals": "venear"}},
                        {"path": "event_event", "operator": {"Equals": "add_vote"}},
                    ]
                },
                severity="info",
                summary_template="House of Stake: Vote cast on proposal",
                dedup_key_template="hos-vote-{transaction_id}",
            ),
        ]
    )


def create_custom_contract_config(
    routing_key: str,
    contract_id: str,
    event_standard: Optional[str] = None,
) -> EventMonitorConfig:
    """
    Example: Monitor any contract's NEP-297 events
    """
    filter_conditions = [
        {"path": "account_id", "operator": {"Equals": contract_id}},
    ]
    
    if event_standard:
        filter_conditions.append(
            {"path": "event_standard", "operator": {"Equals": event_standard}}
        )
    
    return EventMonitorConfig(
        pagerduty_routing_key=routing_key,
        alerts=[
            AlertConfig(
                name=f"Contract Events: {contract_id}",
                event_type="log_nep297",
                filter={"And": filter_conditions},
                severity="warning",
                summary_template=f"Event on {contract_id}: {{event_event}}",
                dedup_key_template=f"{contract_id}-{{transaction_id}}",
            ),
        ]
    )


def create_large_transfer_config(
    routing_key: str,
    token_id: str = "wrap.near",  # wNEAR by default
    min_amount: int = 1000 * 10**24,  # 1000 NEAR in yocto
) -> EventMonitorConfig:
    """
    Example: Monitor large token transfers
    """
    return EventMonitorConfig(
        pagerduty_routing_key=routing_key,
        alerts=[
            AlertConfig(
                name=f"Large Transfer: {token_id}",
                event_type="ft_transfer",
                filter={
                    "And": [
                        {"path": "token_id", "operator": {"Equals": token_id}},
                        # Note: amount filtering may need adjustment based on exact field name
                    ]
                },
                severity="warning",
                summary_template=f"Large {token_id} transfer detected",
                dedup_key_template="transfer-{transaction_id}",
            ),
        ]
    )


def create_function_call_config(
    routing_key: str,
    contract_id: str,
    method_names: Optional[List[str]] = None,
) -> EventMonitorConfig:
    """
    Example: Monitor specific function calls on a contract
    Uses tx_transaction event type
    """
    filter_conditions = [
        {"path": "receiver_id", "operator": {"Equals": contract_id}},
    ]
    
    # Note: For method name filtering, you may need to use the transaction actions
    # This is a simplified example
    
    return EventMonitorConfig(
        pagerduty_routing_key=routing_key,
        alerts=[
            AlertConfig(
                name=f"Function Calls: {contract_id}",
                event_type="tx_transaction",
                filter={"And": filter_conditions},
                severity="warning",
                summary_template=f"Transaction to {contract_id} from {{signer_id}}",
                dedup_key_template=f"{contract_id}-{{transaction_id}}",
            ),
        ]
    )


# =============================================================================
# Main Entry Point
# =============================================================================

async def main():
    """Example usage"""
    routing_key = os.environ.get("PAGERDUTY_ROUTING_KEY")
    if not routing_key:
        raise ValueError("PAGERDUTY_ROUTING_KEY environment variable required")
    
    # Choose your configuration:
    
    # Option 1: House of Stake monitoring (like the Telegram bot)
    config = create_house_of_stake_config(routing_key)
    
    # Option 2: Custom contract monitoring
    # config = create_custom_contract_config(
    #     routing_key,
    #     contract_id="your-contract.near",
    #     event_standard="nep141",  # or None for all events
    # )
    
    # Option 3: Monitor function calls
    # config = create_function_call_config(
    #     routing_key,
    #     contract_id="your-contract.near",
    # )
    
    logger.info(f"Starting NEAR event monitor with {len(config.alerts)} alert(s)")
    await monitor_events(config)


if __name__ == "__main__":
    asyncio.run(main())

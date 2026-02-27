//! Nostr-based signaling for custom mode ICE/QUIC connections.
//!
//! Uses Nostr events (kind 24242) to exchange ICE offers/answers between peers,
//! eliminating the need for manual copy-paste signaling.

use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use log::{info, warn};
use nostr_sdk::prelude::*;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::Mutex;

use super::{ManualAnswer, ManualOffer, ManualReject, ManualRequest};

/// Custom event kind for tunnel-rs signaling (ephemeral range)
fn tunnel_signaling_kind() -> Kind {
    Kind::from_u16(24242)
}

const SIGNALING_TYPE_REQUEST: &str = "tunnel-request";
const SIGNALING_TYPE_OFFER: &str = "tunnel-offer";
const SIGNALING_TYPE_ANSWER: &str = "tunnel-answer";
const SIGNALING_TYPE_REJECT: &str = "tunnel-reject";

/// Errors that can occur during Nostr signaling operations.
#[derive(Debug)]
pub enum SignalingError {
    /// The notification channel was closed (client disconnected).
    ChannelClosed,
    /// Timeout waiting for a message from the peer.
    Timeout,
}

impl SignalingError {
    /// Returns `true` if this error indicates the notification channel was closed.
    pub fn is_channel_closed(&self) -> bool {
        matches!(self, SignalingError::ChannelClosed)
    }
}

impl std::fmt::Display for SignalingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalingError::ChannelClosed => {
                write!(f, "Notification channel closed")
            }
            SignalingError::Timeout => {
                write!(f, "Timeout waiting for message from peer")
            }
        }
    }
}

impl std::error::Error for SignalingError {}

/// Errors that can occur while waiting for an offer.
#[derive(Debug)]
pub enum OfferWaitError {
    /// The session was rejected by the sender.
    Rejected(ManualReject),
    /// The notification channel was closed (client disconnected).
    ChannelClosed,
}

impl std::fmt::Display for OfferWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OfferWaitError::Rejected(reject) => {
                write!(f, "Session rejected: {}", reject.reason)
            }
            OfferWaitError::ChannelClosed => {
                write!(f, "Notification channel closed")
            }
        }
    }
}

impl std::error::Error for OfferWaitError {}

/// Nostr signaling client for ICE exchange.
///
/// # Single-Consumer Constraint
///
/// This struct uses a persistent [`tokio::sync::broadcast::Receiver`] to avoid missing
/// messages between checks. The receiver is protected by a [`Mutex`], which means only
/// one task may read notifications at a time.
///
/// **Callers must not attempt concurrent waiting operations on the persistent receiver.**
/// For example, do not call [`wait_for_fresh_request_forever`](Self::wait_for_fresh_request_forever)
/// and [`try_wait_for_offer_or_rejection`](Self::try_wait_for_offer_or_rejection) from different
/// tasks simultaneously—one will block waiting for the other to release the lock.
///
/// In practice, signaling methods should be called sequentially from a single task
/// (e.g., the session handler loop).
pub struct NostrSignaling {
    client: Client,
    keys: Keys,
    peer_pubkey: PublicKey,
    transfer_id: String,
    relay_urls: Vec<String>,
    /// Persistent notification receiver to avoid missing messages.
    /// Created once after subscription and reused for all receive operations.
    notifications: Mutex<tokio::sync::broadcast::Receiver<RelayPoolNotification>>,
}

impl NostrSignaling {
    /// Create a new Nostr signaling client with user-provided keys.
    ///
    /// # Arguments
    /// * `nsec` - Private key in nsec (bech32) or hex format
    /// * `peer_pubkey_str` - Peer's public key in npub (bech32) or hex format
    /// * `relays` - Optional custom relay URLs (defaults to public relays)
    pub async fn new(
        nsec: &str,
        peer_pubkey_str: &str,
        relays: Option<Vec<String>>,
    ) -> Result<Self> {
        // Parse keys (supports nsec/hex formats)
        let keys = Keys::parse(nsec)
            .context("Failed to parse private key (expected nsec or hex format)")?;

        let peer_pubkey = if peer_pubkey_str.starts_with("npub") {
            PublicKey::from_bech32(peer_pubkey_str)
                .map_err(|e| anyhow::anyhow!("Failed to parse npub: {}", e))
        } else {
            PublicKey::from_hex(peer_pubkey_str)
                .map_err(|e| anyhow::anyhow!("Failed to parse hex pubkey: {}", e))
        }
        .context("Failed to parse peer public key (expected npub or hex format)")?;

        // Derive transfer ID from both pubkeys (deterministic)
        let transfer_id = derive_transfer_id(&keys.public_key(), &peer_pubkey);

        let relay_urls = relays.unwrap_or_else(|| {
            crate::defaults::default_nostr_relays()
                .iter()
                .map(|&relay| relay.to_string())
                .collect()
        });

        let client = Client::new(keys.clone());

        // Create persistent notification receiver BEFORE connecting to relays.
        // This ensures messages are buffered from the moment connections are established,
        // avoiding any race where messages could be sent before we start listening.
        let notifications = Mutex::new(client.notifications());

        // Add and connect to relays
        for relay_url in &relay_urls {
            if let Err(e) = client.add_relay(relay_url).await {
                warn!("Failed to add relay {}: {}", relay_url, e);
            }
        }

        client.connect().await;

        // Wait for relay connections to be established with timeout.
        // This ensures relays are ready before we attempt to send/receive messages.
        // Note: wait_for_connection returns () and continues after timeout even if
        // connections fail. Connection failures will surface as signaling errors later.
        const RELAY_CONNECT_TIMEOUT_SECS: u64 = 10;
        info!(
            "Waiting for Nostr relay connections (timeout: {}s)...",
            RELAY_CONNECT_TIMEOUT_SECS
        );
        client
            .wait_for_connection(Duration::from_secs(RELAY_CONNECT_TIMEOUT_SECS))
            .await;
        info!("Nostr relay connection wait complete");

        Ok(Self {
            client,
            keys,
            peer_pubkey,
            transfer_id,
            relay_urls,
            notifications,
        })
    }

    /// Get the transfer ID
    pub fn transfer_id(&self) -> &str {
        &self.transfer_id
    }

    /// Get our public key in npub format
    pub fn public_key_bech32(&self) -> String {
        self.keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| self.keys.public_key().to_hex())
    }

    /// Get the relay URLs we're connected to
    pub fn relay_urls(&self) -> &[String] {
        &self.relay_urls
    }

    /// Subscribe to incoming signaling events for our pubkey
    pub async fn subscribe(&self) -> Result<()> {
        let filter = Filter::new()
            .kind(tunnel_signaling_kind())
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::T),
                self.transfer_id.clone(),
            )
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::P),
                self.keys.public_key().to_hex(),
            );

        self.client
            .subscribe(filter, None)
            .await
            .context("Failed to subscribe to signaling events")?;

        Ok(())
    }

    /// Publish an ICE offer to the peer
    pub async fn publish_offer(&self, offer: &ManualOffer) -> Result<()> {
        let json = serde_json::to_string(offer)?;
        let content = URL_SAFE_NO_PAD.encode(json.as_bytes());

        let event = self.create_signaling_event(SIGNALING_TYPE_OFFER, &content)?;

        self.client
            .send_event(&event)
            .await
            .context("Failed to publish ICE offer")?;

        info!("Published ICE offer to Nostr relays");
        Ok(())
    }

    /// Publish an ICE answer to the peer
    pub async fn publish_answer(&self, answer: &ManualAnswer) -> Result<()> {
        let json = serde_json::to_string(answer)?;
        let content = URL_SAFE_NO_PAD.encode(json.as_bytes());

        let event = self.create_signaling_event(SIGNALING_TYPE_ANSWER, &content)?;

        self.client
            .send_event(&event)
            .await
            .context("Failed to publish ICE answer")?;

        info!("Published ICE answer to Nostr relays");
        Ok(())
    }

    /// Publish an ICE request to the peer (receiver -> sender to initiate session)
    pub async fn publish_request(&self, request: &ManualRequest) -> Result<()> {
        let json = serde_json::to_string(request)?;
        let content = URL_SAFE_NO_PAD.encode(json.as_bytes());

        let event = self.create_signaling_event(SIGNALING_TYPE_REQUEST, &content)?;

        self.client
            .send_event(&event)
            .await
            .context("Failed to publish ICE request")?;

        info!("Published ICE request to Nostr relays");
        Ok(())
    }

    /// Publish a session rejection to the peer (sender -> receiver when at capacity)
    pub async fn publish_reject(&self, reject: &ManualReject) -> Result<()> {
        let json = serde_json::to_string(reject)?;
        let content = URL_SAFE_NO_PAD.encode(json.as_bytes());

        let event = self.create_signaling_event(SIGNALING_TYPE_REJECT, &content)?;

        self.client
            .send_event(&event)
            .await
            .context("Failed to publish session rejection")?;

        info!(
            "Published session rejection to Nostr relays (session: {})",
            reject.session_id
        );
        Ok(())
    }

    /// Wait for a fresh request from the peer indefinitely.
    /// For multi-session senders that should run forever.
    /// Rejects requests older than `max_age_secs` seconds.
    pub async fn wait_for_fresh_request_forever(&self, max_age_secs: u64) -> Result<ManualRequest> {
        self.wait_for_fresh_request_inner(max_age_secs, None).await
    }

    /// Inner implementation for waiting for fresh requests.
    /// If `timeout_secs` is None, waits indefinitely.
    ///
    /// Uses the persistent notification receiver to avoid missing messages.
    async fn wait_for_fresh_request_inner(
        &self,
        max_age_secs: u64,
        timeout_secs: Option<u64>,
    ) -> Result<ManualRequest> {
        match timeout_secs {
            Some(t) => info!(
                "Waiting for {} from peer (timeout: {}s, max age: {}s)...",
                SIGNALING_TYPE_REQUEST, t, max_age_secs
            ),
            None => info!(
                "Waiting for {} from peer (max age: {}s)...",
                SIGNALING_TYPE_REQUEST, max_age_secs
            ),
        }

        let deadline = timeout_secs.map(|t| tokio::time::Instant::now() + Duration::from_secs(t));
        let mut notifications = self.notifications.lock().await;

        loop {
            // Early deadline check before any other work
            if let Some(d) = deadline {
                if tokio::time::Instant::now() >= d {
                    return Err(SignalingError::Timeout.into());
                }
            }

            // Compute wait duration: min(remaining until deadline, 1s), or 1s if no deadline
            let wait_duration = if let Some(d) = deadline {
                let remaining = d.saturating_duration_since(tokio::time::Instant::now());
                remaining.min(Duration::from_secs(1))
            } else {
                Duration::from_secs(1)
            };

            match tokio::time::timeout(wait_duration, notifications.recv()).await {
                Ok(Ok(RelayPoolNotification::Event { event, .. })) => {
                    if let Some(request) =
                        self.try_parse_event::<ManualRequest>(&event, SIGNALING_TYPE_REQUEST)
                    {
                        // Check if request is fresh
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let age = now.saturating_sub(request.timestamp);
                        if age <= max_age_secs {
                            info!(
                                "Received fresh {} from peer (age: {}s)",
                                SIGNALING_TYPE_REQUEST, age
                            );
                            return Ok(request);
                        } else {
                            info!(
                                "Ignoring stale request (age: {}s > max {}s)",
                                age, max_age_secs
                            );
                        }
                    }
                }
                Ok(Ok(_)) => continue,
                Ok(Err(recv_err)) => {
                    match recv_err {
                        RecvError::Closed => {
                            return Err(SignalingError::ChannelClosed.into());
                        }
                        RecvError::Lagged(skipped) => {
                            warn!(
                                "Warning: Notification message receiver lagged, skipped {} messages; draining buffer...",
                                skipped
                            );
                            // Drain buffered messages and check for fresh request
                            match self
                                .drain_and_find_fresh_request(&mut notifications, max_age_secs)
                            {
                                Ok(Some(request)) => return Ok(request),
                                Ok(None) => {} // No matching request found, continue waiting
                                Err(SignalingError::ChannelClosed) => {
                                    return Err(SignalingError::ChannelClosed.into());
                                }
                                Err(e) => return Err(e.into()),
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
    }

    /// Check a single event for a fresh request.
    fn check_event_for_fresh_request(
        &self,
        event: &Event,
        max_age_secs: u64,
    ) -> Option<ManualRequest> {
        if let Some(request) = self.try_parse_event::<ManualRequest>(event, SIGNALING_TYPE_REQUEST)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let age = now.saturating_sub(request.timestamp);
            if age <= max_age_secs {
                info!(
                    "Received fresh {} from peer (age: {}s)",
                    SIGNALING_TYPE_REQUEST, age
                );
                return Some(request);
            }
            info!(
                "Ignoring stale request (age: {}s > max {}s)",
                age, max_age_secs
            );
        }
        None
    }

    /// Drain buffered messages looking for a fresh request.
    ///
    /// Returns:
    /// - `Ok(Some(request))` if a fresh request is found
    /// - `Ok(None)` if buffer is empty (no match found)
    /// - `Err(SignalingError::ChannelClosed)` if the channel was closed
    fn drain_and_find_fresh_request(
        &self,
        notifications: &mut tokio::sync::broadcast::Receiver<RelayPoolNotification>,
        max_age_secs: u64,
    ) -> Result<Option<ManualRequest>, SignalingError> {
        loop {
            match notifications.try_recv() {
                Ok(RelayPoolNotification::Event { event, .. }) => {
                    if let Some(request) = self.check_event_for_fresh_request(&event, max_age_secs)
                    {
                        return Ok(Some(request));
                    }
                }
                Ok(_) => continue,
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Closed) => {
                    return Err(SignalingError::ChannelClosed);
                }
                Err(TryRecvError::Lagged(more_skipped)) => {
                    warn!(
                        "Warning: Additional {} messages skipped while draining",
                        more_skipped
                    );
                    continue;
                }
            }
        }
    }

    /// Wait for an offer from the peer, also checking for rejections.
    ///
    /// # Receiver Strategy: Persistent (Mutex-locked)
    ///
    /// This method uses the persistent `self.notifications` receiver which is protected
    /// by a Mutex. This receiver was created when `NostrSignaling` was constructed and
    /// never misses messages sent after construction.
    ///
    /// **Trade-offs:**
    /// - **Guaranteed reception**: Messages are never missed because the receiver exists
    ///   from the start of the signaling session.
    /// - **Serialized access**: The Mutex ensures only one task can wait at a time,
    ///   preventing message consumption conflicts.
    /// - **Blocking**: Other tasks needing this receiver must wait for the lock.
    ///
    /// Use this method when you need guaranteed message reception and can tolerate
    /// serialized access (e.g., receiver waiting for offers from a single sender).
    ///
    /// # Returns
    /// - `Ok(Some(offer))` if an offer matching `session_id` is received
    /// - `Ok(None)` on timeout
    /// - `Err(OfferWaitError::Rejected(reject))` if a rejection matching `session_id` is received
    /// - `Err(OfferWaitError::ChannelClosed)` if the notification channel was closed
    pub async fn try_wait_for_offer_or_rejection(
        &self,
        session_id: &str,
        timeout_secs: u64,
    ) -> Result<Option<ManualOffer>, OfferWaitError> {
        let mut notifications = self.notifications.lock().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None); // Timeout
            }
            let wait_duration = remaining.min(Duration::from_secs(1));

            match tokio::time::timeout(wait_duration, notifications.recv()).await {
                Ok(Ok(RelayPoolNotification::Event { event, .. })) => {
                    if let Some(result) =
                        self.check_event_for_offer_or_rejection(&event, session_id)
                    {
                        return result;
                    }
                }
                Ok(Ok(_)) => continue,
                Ok(Err(recv_err)) => match recv_err {
                    RecvError::Closed => {
                        return Err(OfferWaitError::ChannelClosed);
                    }
                    RecvError::Lagged(skipped) => {
                        warn!(
                            "Warning: Notification message receiver lagged, skipped {} messages; draining buffer...",
                            skipped
                        );
                        // Drain buffered messages and check for offer/rejection
                        if let Some(result) =
                            self.drain_and_find_offer_or_rejection(&mut notifications, session_id)
                        {
                            return result;
                        }
                        // No matching offer/rejection found in buffer, continue waiting
                    }
                },
                Err(_) => continue, // Timeout elapsed, loop again
            }
        }
    }

    /// Check a single event for an offer or rejection matching the session ID.
    fn check_event_for_offer_or_rejection(
        &self,
        event: &Event,
        session_id: &str,
    ) -> Option<Result<Option<ManualOffer>, OfferWaitError>> {
        // Check for offer first
        if let Some(offer) = self.try_parse_event::<ManualOffer>(event, SIGNALING_TYPE_OFFER) {
            if offer.session_id.as_deref() == Some(session_id) {
                info!("Received {} from peer", SIGNALING_TYPE_OFFER);
                return Some(Ok(Some(offer)));
            }
            info!("Ignoring offer with mismatched session ID (stale event)");
        }
        // Check for rejection
        if let Some(reject) = self.try_parse_event::<ManualReject>(event, SIGNALING_TYPE_REJECT) {
            if reject.session_id == session_id {
                info!("Received {} from peer", SIGNALING_TYPE_REJECT);
                return Some(Err(OfferWaitError::Rejected(reject)));
            }
            info!("Ignoring reject with mismatched session ID (stale event)");
        }
        None
    }

    /// Drain buffered messages looking for an offer or rejection matching the session ID.
    fn drain_and_find_offer_or_rejection(
        &self,
        notifications: &mut tokio::sync::broadcast::Receiver<RelayPoolNotification>,
        session_id: &str,
    ) -> Option<Result<Option<ManualOffer>, OfferWaitError>> {
        loop {
            match notifications.try_recv() {
                Ok(RelayPoolNotification::Event { event, .. }) => {
                    if let Some(result) =
                        self.check_event_for_offer_or_rejection(&event, session_id)
                    {
                        return Some(result);
                    }
                }
                Ok(_) => continue,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Closed) => {
                    return Some(Err(OfferWaitError::ChannelClosed));
                }
                Err(TryRecvError::Lagged(more_skipped)) => {
                    warn!(
                        "Warning: Additional {} messages skipped while draining",
                        more_skipped
                    );
                    continue;
                }
            }
        }
    }

    /// Try to parse an event as the expected message type.
    /// Returns Some(payload) if the event matches our criteria and can be parsed.
    fn try_parse_event<T: for<'de> serde::Deserialize<'de>>(
        &self,
        event: &Event,
        expected_type: &str,
    ) -> Option<T> {
        // Verify this is from our peer
        if event.pubkey != self.peer_pubkey {
            return None;
        }

        // Check transfer ID
        let is_our_transfer = event.tags.iter().any(|t| {
            t.kind() == TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::T))
                && t.content() == Some(&self.transfer_id)
        });
        if !is_our_transfer {
            return None;
        }

        // Check event type
        let event_type = event
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::custom("type"))
            .and_then(|t| t.content());

        if event_type != Some(expected_type) {
            return None;
        }

        // Decode content
        let decoded = URL_SAFE_NO_PAD.decode(event.content.as_bytes()).ok()?;
        serde_json::from_slice(&decoded).ok()
    }

    fn create_signaling_event(&self, event_type: &str, content: &str) -> Result<Event> {
        let tags = vec![
            Tag::custom(
                TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::T)),
                vec![self.transfer_id.clone()],
            ),
            Tag::custom(
                TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::P)),
                vec![self.peer_pubkey.to_hex()],
            ),
            Tag::custom(TagKind::custom("type"), vec![event_type.to_string()]),
        ];

        EventBuilder::new(tunnel_signaling_kind(), content)
            .tags(tags)
            .sign_with_keys(&self.keys)
            .context("Failed to sign signaling event")
    }

    /// Disconnect from all relays
    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }
}

/// Derive a deterministic transfer ID from two public keys.
/// This ensures both peers calculate the same ID without prior coordination.
fn derive_transfer_id(pk1: &PublicKey, pk2: &PublicKey) -> String {
    let mut keys = [pk1.to_hex(), pk2.to_hex()];
    keys.sort(); // Deterministic ordering

    let mut hasher = Sha256::new();
    hasher.update(keys[0].as_bytes());
    hasher.update(keys[1].as_bytes());
    let result = hasher.finalize();

    hex::encode(&result[..16]) // 16 bytes = 32 hex chars
}

/// Generate a new Nostr keypair
pub fn generate_keypair() -> Keys {
    Keys::generate()
}

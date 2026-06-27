//! WebSocket client built on the ESP-IDF `esp_websocket_client` component
//! (via `esp_idf_svc::ws::client`).
//!
//! The ESP-IDF client is callback/event-driven: events arrive on an internal
//! IDF task. To keep the public API tokio-compatible (so `recv()` can be used
//! inside the `tokio::select!` in `app.rs`), we bridge those events into a
//! tokio channel.
//!
//! TLS (`wss://`) is handled by ESP-IDF's built-in mbedTLS, using the default
//! CA certificate bundle (`CONFIG_MBEDTLS_CERTIFICATE_BUNDLE_DEFAULT_FULL=y`),
//! so we can connect to any server whose certificate chains to a public root
//! CA without hardcoding a server certificate.

use std::time::Duration;

use esp_idf_svc::io::EspIOError;
use esp_idf_svc::sys::esp_crt_bundle_attach;
use esp_idf_svc::ws::client::{
    EspWebSocketClient, EspWebSocketClientConfig, FrameType, WebSocketEvent, WebSocketEventType,
};
use tokio::sync::{mpsc, oneshot};

use crate::protocol;

/// Per-frame buffer size for the ESP-IDF websocket client (tx + rx).
///
/// Tune this to be >= the largest single message so it goes out / comes in as
/// one frame. The biggest payload is an audio chunk = `feed_chunksize * 2`
/// bytes (i16 samples); the AFE prints `audio chunksize: N` at boot. 8 KiB
/// comfortably covers typical ESP-SR chunks; raise it if logs show larger.
const WS_BUFFER_SIZE: usize = 8 * 1024;

/// Events forwarded from the ESP-IDF websocket callback into the async world.
enum WsEvent {
    Connected,
    Disconnected,
    Closed,
    Message(protocol::ServerMessage),
}

pub struct Server {
    #[allow(dead_code)]
    pub uri: String,
    client: EspWebSocketClient<'static>,
    rx: mpsc::UnboundedReceiver<WsEvent>,
}

impl Server {
    pub async fn new(uri: String) -> anyhow::Result<Self> {
        let uri = if cfg!(feature = "max2") {
            format!("{}?pty=false&img=true&width=320&height=168", uri)
        } else {
            format!("{}?pty=false&img=true&width=288&height=80", uri)
        };

        // Messages and connection-state changes flow through here so that
        // `recv()` can be awaited inside a `tokio::select!`.
        let (tx, rx) = mpsc::unbounded_channel::<WsEvent>();
        // Signaled exactly once when the connection is first established
        // (or definitively closed), so `new()` can report connect failures.
        let (conn_tx, conn_rx) = oneshot::channel::<bool>();
        let mut conn_tx = Some(conn_tx);

        // Rely on ESP-IDF's default CA bundle for `wss://` validation.
        //
        // `buffer_size` is the per-frame chunk size for the ESP-IDF client. Sends
        // larger than this are transparently fragmented into continuation frames
        // (the server reassembles them), so it is NOT a hard message limit. We
        // raise it above the largest payload (audio chunks, ~feed_chunksize*2
        // bytes; logged at boot as `audio chunksize: N`) so that the common case
        // goes out as a single frame, matching the old tokio_websockets behaviour.
        let config = EspWebSocketClientConfig {
            crt_bundle_attach: Some(esp_crt_bundle_attach),
            buffer_size: WS_BUFFER_SIZE,
            ..Default::default()
        };

        // `new()` only *starts* the client; the connection completes
        // asynchronously. Auto-reconnect stays enabled (the default) so the
        // device survives transient network drops.
        let client = EspWebSocketClient::new(&uri, &config, Duration::from_secs(10), move |event| {
            Self::handle_event(&tx, &mut conn_tx, event);
        })?;

        // Block until the connection is actually up (or fails/times out),
        // mirroring the old blocking connect semantics used by `app.rs`.
        let connected = tokio::time::timeout(Duration::from_secs(15), conn_rx)
            .await
            .map_err(|_| anyhow::anyhow!("Timeout waiting for websocket to connect"))?
            .map_err(|_| anyhow::anyhow!("WebSocket event loop dropped before connect"))?;

        if !connected {
            anyhow::bail!("WebSocket connection closed before it could be established");
        }

        Ok(Self { uri, client, rx })
    }

    fn handle_event(
        tx: &mpsc::UnboundedSender<WsEvent>,
        conn_tx: &mut Option<oneshot::Sender<bool>>,
        event: &Result<WebSocketEvent<'_>, EspIOError>,
    ) {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                log::error!("WebSocket error event: {:?}", e);
                return;
            }
        };

        match event.event_type {
            WebSocketEventType::BeforeConnect => {
                log::debug!("WebSocket before connect");
            }
            WebSocketEventType::Connected => {
                log::info!("WebSocket connected");
                if let Some(sender) = conn_tx.take() {
                    let _ = sender.send(true);
                }
                let _ = tx.send(WsEvent::Connected);
            }
            WebSocketEventType::Disconnected => {
                log::warn!("WebSocket disconnected (will auto-reconnect)");
                let _ = tx.send(WsEvent::Disconnected);
            }
            WebSocketEventType::Close(reason) => {
                log::warn!("WebSocket close frame received: {:?}", reason);
            }
            WebSocketEventType::Closed => {
                log::warn!("WebSocket closed");
                if let Some(sender) = conn_tx.take() {
                    let _ = sender.send(false);
                }
                let _ = tx.send(WsEvent::Closed);
            }
            WebSocketEventType::Text(text) => match protocol::ServerMessage::from_json(text) {
                Ok(m) => {
                    let _ = tx.send(WsEvent::Message(m));
                }
                Err(e) => {
                    log::error!("Failed to parse text message: {:?}, error: {:?}", text, e);
                }
            },
            WebSocketEventType::Binary(data) => {
                match protocol::ServerMessage::from_msgpack(data) {
                    Ok(m) => {
                        let _ = tx.send(WsEvent::Message(m));
                    }
                    Err(e) => {
                        log::error!("Failed to parse binary message: {:?}, error: {:?}", data, e);
                    }
                }
            }
            WebSocketEventType::Ping | WebSocketEventType::Pong => {
                // Handled implicitly by the ESP-IDF client.
            }
        }
    }

    pub async fn send(&mut self, msg: protocol::ClientMessage) -> anyhow::Result<()> {
        let payload = msg.to_msgpack()?;
        self.client
            .send(FrameType::Binary(false), &payload)
            .map_err(|e| anyhow::anyhow!("WebSocket send failed: {:?}", e))?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<protocol::ServerMessage> {
        loop {
            match self.rx.recv().await {
                Some(WsEvent::Message(msg)) => return Some(msg),
                Some(WsEvent::Connected) | Some(WsEvent::Disconnected) => {
                    // State transitions are logged in the callback; the client
                    // auto-reconnects on its own, so keep waiting for messages.
                }
                Some(WsEvent::Closed) | None => return None,
            }
        }
    }
}

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio_websockets::Message;

use crate::protocol;

pub struct Server {
    pub uri: String,
    timeout: std::time::Duration,
    ws: tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl Server {
    pub async fn new(uri: String) -> anyhow::Result<Self> {
        let uri = format!("{}?pty=false&img=true&width=288&height=80", uri);
        let (ws, _resp) = tokio_websockets::ClientBuilder::new()
            .uri(&uri)?
            .connect()
            .await?;

        let timeout = std::time::Duration::from_secs(30);

        Ok(Self { uri, timeout, ws })
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.ws.close().await?;
        Ok(())
    }

    pub async fn reconnect(&mut self) -> anyhow::Result<()> {
        let (ws, _resp) = tokio_websockets::ClientBuilder::new()
            .uri(&self.uri)?
            .connect()
            .await?;
        self.ws = ws;
        Ok(())
    }

    pub fn set_timeout(&mut self, timeout: std::time::Duration) {
        self.timeout = timeout;
    }

    pub async fn send(&mut self, msg: protocol::ClientMessage) -> anyhow::Result<()> {
        let msg = Message::binary(msg.to_msgpack()?);
        tokio::time::timeout(self.timeout, self.ws.send(msg))
            .map_err(|_| anyhow::anyhow!("Timeout sending message"))
            .await??;
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<protocol::ServerMessage> {
        loop {
            match self.ws.next().await {
                Some(Ok(msg)) => {
                    if msg.is_close() {
                        log::warn!("WebSocket connection closed by server");
                        return None;
                    } else if msg.is_binary() {
                        let data = msg.as_payload();

                        let msg = match protocol::ServerMessage::from_msgpack(data) {
                            Ok(m) => m,
                            Err(e) => {
                                log::error!(
                                    "Failed to parse binary message: {:?}, error: {:?}",
                                    data,
                                    e
                                );
                                continue;
                            }
                        };
                        return Some(msg);
                    } else if msg.is_text() {
                        log::warn!("Received unexpected text message: {:?}", msg);
                        let text = msg.as_text().unwrap();
                        let msg = match protocol::ServerMessage::from_json(text) {
                            Ok(m) => m,
                            Err(e) => {
                                log::error!(
                                    "Failed to parse text message: {:?}, error: {:?}",
                                    text,
                                    e
                                );
                                continue;
                            }
                        };
                        return Some(msg);
                    } else if msg.is_ping() {
                        // ignore ping frames
                    } else {
                        log::warn!("Received unsupported message type: {:?}", msg);
                        continue;
                    }
                }
                Some(Err(e)) => {
                    log::error!("WebSocket receive error: {:?}", e);
                    return None;
                }
                None => {
                    log::warn!("WebSocket connection closed by server");
                    return None;
                }
            }
        }
    }
}

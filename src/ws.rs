use futures_util::{SinkExt, TryFutureExt};
use tokio_websockets::Message;

pub struct Server {
    pub uri: String,
    timeout: std::time::Duration,
    ws: tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl Server {
    pub async fn new(uri: String) -> anyhow::Result<Self> {
        let uri = format!("{}?record=true", uri);
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

    pub async fn send(&mut self, msg: Message) -> anyhow::Result<()> {
        tokio::time::timeout(self.timeout, self.ws.send(msg))
            .map_err(|_| anyhow::anyhow!("Timeout sending message"))
            .await??;
        Ok(())
    }
}

//! MQTT 传输:对接 vibetty 的 MQTT 桥接协议。
//!
//! 与旧 `ws::Server` 暴露相同的 `send(ClientMessage)` / `recv() -> Option<ServerMessage>`
//! 接口,内部封装:broker 连接、presence 服务发现、按 topic 分发、screen 分片重组。
//!
//! 协议契约见 `vibetty/docs/esp32-mqtt-integration.md`。实例前缀
//! `P = {user}/{device}/{pid}/vibetty`,其中 device(PC 机器指纹)与 pid(每次重启变)
//! ESP32 都无法预知,必须先通过 presence 发现。

use std::collections::HashMap;
use std::time::Duration;

use embedded_svc::mqtt::client::{Details, EventPayload, QoS};
use esp_idf_svc::mqtt::client::{
    EspAsyncMqttClient, EspAsyncMqttConnection, MqttClientConfiguration,
};

use crate::protocol::{ClientMessage, ImageFormat, ScreenImageChunk, ServerMessage};

/// 宽通配的 presence 发现 topic(匹配正好 4 段:`{user}/{device}/{pid}/vibetty`)。
const DISCOVERY_TOPIC: &str = "+/+/+/vibetty";

/// 单张 screen 重组上限,超过则丢弃防 OOM。
const REASSEMBLY_MAX: usize = 256 * 1024;

pub struct MqttServer {
    client: EspAsyncMqttClient,
    conn: EspAsyncMqttConnection,
    /// recv 写入:当前想跟随的实例 prefix(由 presence 决定)。
    desired_prefix: Option<String>,
    /// flush_pending 写入:已经 subscribe 了 pty_out/screen 的实例 prefix。
    subscribed_prefix: Option<String>,
    /// 多实例择优:跟随 presence ts 最大的实例。
    presence_ts: u64,
    /// screen 分片重组缓冲,key = topic。
    reassembly: HashMap<String, Vec<u8>>,
}

/// vibetty presence 公告。
#[derive(serde::Deserialize)]
struct Presence {
    prefix: String,
    #[allow(dead_code)]
    client_id: String,
    ts: u64,
}

impl MqttServer {
    /// `uri` 形如 `mqtt://user:pass@host:port` 或 `mqtts://user:pass@host:port`。
    /// `client_id` 需在 broker 内唯一(调用方用 MAC 等稳定来源生成)。
    pub async fn new(uri: &str, client_id: &str) -> anyhow::Result<Self> {
        let BrokerInfo {
            broker_url,
            username,
            password,
            use_tls,
        } = parse_broker_uri(uri)?;

        let mut conf = MqttClientConfiguration {
            client_id: Some(client_id),
            username: Some(&username),
            password: Some(&password),
            buffer_size: 64 * 1024, // 收,需装下整张 screen
            out_buffer_size: 8 * 1024,
            keep_alive_interval: Some(Duration::from_secs(20)),
            reconnect_timeout: Some(Duration::from_secs(10)),
            network_timeout: Duration::from_secs(30),
            ..Default::default()
        };
        if use_tls {
            // sdkconfig 已开启 MBEDTLS_CERTIFICATE_BUNDLE
            conf.crt_bundle_attach = Some(esp_idf_svc::sys::esp_crt_bundle_attach);
        }

        let (mut client, mut conn) = EspAsyncMqttClient::new(&broker_url, &conf)
            .map_err(|e| anyhow::anyhow!("EspAsyncMqttClient::new failed: {e:?}"))?;

        // esp-mqtt 连接在内部 task 异步建立,必须等到 Connected 再 subscribe。
        let wait = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match conn.next().await {
                    Ok(ev) => {
                        if let EventPayload::Connected(session_present) = ev.payload() {
                            log::info!("MQTT connected (session_present={session_present})");
                            return;
                        }
                    }
                    Err(e) => log::warn!("MQTT conn event error before connect: {e:?}"),
                }
            }
        })
        .await;
        wait.map_err(|_| anyhow::anyhow!("MQTT connect timeout (30s)"))?;

        // Discovery:retained → 一连上立即收到所有现存实例的 presence。
        client
            .subscribe(DISCOVERY_TOPIC, QoS::AtLeastOnce)
            .await
            .map_err(|e| anyhow::anyhow!("subscribe discovery failed: {e:?}"))?;
        log::info!("Subscribed discovery topic: {DISCOVERY_TOPIC}");

        Ok(Self {
            client,
            conn,
            desired_prefix: None,
            subscribed_prefix: None,
            presence_ts: 0,
            reassembly: HashMap::new(),
        })
    }

    /// 把 `desired_prefix` 落实为 subscribe。
    ///
    /// **必须在 `select!` 之外被 await**:`client.subscribe/unsubscribe` 的 future 若被
    /// select 中途 drop,会让 client 命令通道进入未定义状态甚至死锁。
    pub async fn flush_pending(&mut self) -> anyhow::Result<()> {
        if self.desired_prefix == self.subscribed_prefix {
            return Ok(());
        }

        // 先退订旧实例的输出通道
        if let Some(old) = self.subscribed_prefix.take() {
            log::info!("Unsubscribing old instance output: {old}");
            let _ = self.client.unsubscribe(&format!("{old}/pty_out")).await;
            let _ = self.client.unsubscribe(&format!("{old}/screen")).await;
            self.reassembly.clear();
        }

        // 再订阅新实例
        if let Some(new) = self.desired_prefix.clone() {
            log::info!("Subscribing instance output: {new}");
            self.client
                .subscribe(&format!("{new}/pty_out"), QoS::AtLeastOnce)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe pty_out failed: {e:?}"))?;
            self.client
                .subscribe(&format!("{new}/screen"), QoS::AtLeastOnce)
                .await
                .map_err(|e| anyhow::anyhow!("subscribe screen failed: {e:?}"))?;
            self.subscribed_prefix = Some(new);
        }

        Ok(())
    }

    /// 阻塞等待下一条需要 app 处理的消息(presence / 连接事件在内部消化)。
    /// 返回 `None` 表示连接已关闭。
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        loop {
            // 先把事件数据拷贝成 owned,并把 `ev`(借用 self.conn)的作用域限制在内层
            // block 里 —— 否则下面 `self.reassemble_screen(&mut self)` 会与 ev 的借用冲突。
            let received: Option<(String, Vec<u8>, Details)> = {
                let ev = match self.conn.next().await {
                    Ok(ev) => ev,
                    Err(e) => {
                        log::error!("MQTT conn.next() error: {e:?}");
                        return None;
                    }
                };
                match ev.payload() {
                    EventPayload::Received {
                        topic,
                        data,
                        details,
                        ..
                    } => Some((topic.unwrap_or("").to_string(), data.to_vec(), details)),
                    EventPayload::Connected(sp) => {
                        log::info!("MQTT (re)connected, session={sp}");
                        None
                    }
                    EventPayload::Disconnected => {
                        log::warn!("MQTT disconnected");
                        None
                    }
                    EventPayload::Error(e) => {
                        log::error!("MQTT event error: {e:?}");
                        None
                    }
                    other => {
                        log::debug!("MQTT event: {other:?}");
                        None
                    }
                }
            };

            let (topic, data, details) = match received {
                Some(r) => r,
                None => continue,
            };

            // presence:正好 4 段且以 /vibetty 结尾
            if topic.matches('/').count() == 3 && topic.ends_with("/vibetty") {
                if data.is_empty() {
                    // LWT:实例下线(空 payload = 删除 retained)
                    log::info!("Instance offline (LWT): {topic}");
                    if self.desired_prefix.as_deref() == Some(topic.as_str()) {
                        self.desired_prefix = None;
                    }
                } else {
                    match serde_json::from_slice::<Presence>(&data) {
                        Ok(p) => {
                            if p.ts >= self.presence_ts
                                && self.desired_prefix.as_deref() != Some(p.prefix.as_str())
                            {
                                log::info!("Presence switch -> {}", p.prefix);
                                self.desired_prefix = Some(p.prefix);
                                self.presence_ts = p.ts;
                            }
                        }
                        Err(e) => log::warn!("Bad presence JSON: {e}"),
                    }
                }
                continue;
            }

            if topic.ends_with("/pty_out") {
                return Some(ServerMessage::PtyOutput(data));
            }

            if topic.ends_with("/screen") {
                if let Some(msg) = self.reassemble_screen(&topic, &data, details) {
                    return Some(msg);
                }
                continue;
            }

            log::debug!("Ignored topic: {topic}");
        }
    }

    /// screen 按整张投递;若超 buffer 被分片则按 topic 重组,`Complete` 时产出。
    fn reassemble_screen(
        &mut self,
        topic: &str,
        data: &[u8],
        details: Details,
    ) -> Option<ServerMessage> {
        match details {
            Details::Complete => {
                // buffer 非空表示之前累积过分片,拼上最后一块
                let complete = if let Some(buf) = self.reassembly.remove(topic) {
                    let mut v = buf;
                    v.extend_from_slice(data);
                    v
                } else {
                    data.to_vec()
                };
                Some(ServerMessage::ScreenImage(ScreenImageChunk {
                    format: detect_format(&complete),
                    is_last: true,
                    data: complete,
                }))
            }
            Details::InitialChunk(_) => {
                let buf = self.reassembly.entry(topic.to_string()).or_default();
                buf.clear();
                buf.extend_from_slice(data);
                if buf.len() > REASSEMBLY_MAX {
                    log::error!("Screen reassembly exceeded {REASSEMBLY_MAX} bytes, dropped");
                    buf.clear();
                }
                None
            }
            Details::SubsequentChunk(_) => {
                let buf = self.reassembly.entry(topic.to_string()).or_default();
                buf.extend_from_slice(data);
                if buf.len() > REASSEMBLY_MAX {
                    log::error!("Screen reassembly exceeded {REASSEMBLY_MAX} bytes, dropped");
                    buf.clear();
                }
                None
            }
        }
    }

    /// 发送按键 / 控制消息。`PtyInput` 走 `{P}/pty_in` raw;`VoiceInput*` 在 MQTT 上
    /// 不支持(无语音通道);其余走 `{P}/control` 的 JSON(serde tag 已匹配协议)。
    pub async fn send(&mut self, msg: ClientMessage) -> anyhow::Result<()> {
        let prefix = self
            .subscribed_prefix
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No vibetty instance subscribed yet"))?;

        match msg {
            ClientMessage::PtyInput(bytes) => {
                self.client
                    .publish(
                        &format!("{prefix}/pty_in"),
                        QoS::AtLeastOnce,
                        false,
                        &bytes[..],
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("publish pty_in failed: {e:?}"))?;
            }
            ClientMessage::VoiceInputStart(_)
            | ClientMessage::VoiceInputChunk(_)
            | ClientMessage::VoiceInputEnd(_) => {
                log::warn!("Voice messages are not supported over MQTT, ignored");
            }
            other => {
                let json = other.to_json()?;
                self.client
                    .publish(
                        &format!("{prefix}/control"),
                        QoS::AtLeastOnce,
                        false,
                        json.as_bytes(),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("publish control failed: {e:?}"))?;
            }
        }
        Ok(())
    }
}

/// 据 magic bytes 判断图片格式。
fn detect_format(data: &[u8]) -> ImageFormat {
    if data.starts_with(b"\x89PNG") {
        ImageFormat::Png
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        ImageFormat::Jpeg
    } else {
        log::warn!("Unknown screen image magic bytes, assuming PNG");
        ImageFormat::Png
    }
}

struct BrokerInfo {
    broker_url: String,
    username: String,
    password: String,
    use_tls: bool,
}

/// 手写解析 `mqtt://user:pass@host:port` / `mqtts://user:pass@host:port`。
///
/// 不用 `http` crate(它不暴露 userinfo),也不引入 `url` 依赖。
fn parse_broker_uri(uri: &str) -> anyhow::Result<BrokerInfo> {
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("MQTT URI missing '://': {uri}"))?;
    let use_tls = match scheme {
        "mqtt" => false,
        "mqtts" => true,
        other => anyhow::bail!("Unsupported MQTT scheme '{other}', use mqtt:// or mqtts://"),
    };

    // rest = [user[:pass]@]host[:port][/...]
    let (userinfo, hostport) = match rest.rfind('@') {
        Some(idx) => (Some(&rest[..idx]), &rest[idx + 1..]),
        None => (None, rest),
    };
    // 去掉可能尾随的路径
    let hostport = hostport.split('/').next().unwrap_or(hostport);

    let (username, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((user, pass)) => (user.to_string(), pass.to_string()),
            None => (u.to_string(), String::new()),
        },
        None => {
            anyhow::bail!(
                "MQTT URI missing username:password (expected mqtt://user:pass@host:port)"
            )
        }
    };

    let default_port = if use_tls { 8883 } else { 1883 };
    let port = hostport
        .rsplit_once(':')
        .map(|(_, p)| p.parse::<u16>().ok())
        .flatten()
        .unwrap_or(default_port);
    let host = hostport
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(hostport);

    if host.is_empty() {
        anyhow::bail!("MQTT URI missing host");
    }

    let scheme_str = if use_tls { "mqtts" } else { "mqtt" };
    Ok(BrokerInfo {
        broker_url: format!("{scheme_str}://{host}:{port}"),
        username,
        password,
        use_tls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mqtt_plain() {
        let i = parse_broker_uri("mqtt://alice:secret@broker.example.com:1883").unwrap();
        assert_eq!(i.broker_url, "mqtt://broker.example.com:1883");
        assert_eq!(i.username, "alice");
        assert_eq!(i.password, "secret");
        assert!(!i.use_tls);
    }

    #[test]
    fn parse_mqtts_default_port() {
        let i = parse_broker_uri("mqtts://bob:p@host.io").unwrap();
        assert_eq!(i.broker_url, "mqtts://host.io:8883");
        assert!(i.use_tls);
    }

    #[test]
    fn parse_password_with_special_chars() {
        // 密码里含 '@' 会干扰 rfind('@');这里只验证无 @ 的常见情形
        let i = parse_broker_uri("mqtt://u:p-a-ss@1.2.3.4:1883").unwrap();
        assert_eq!(i.username, "u");
        assert_eq!(i.password, "p-a-ss");
        assert_eq!(i.broker_url, "mqtt://1.2.3.4:1883");
    }

    #[test]
    fn detect_png_jpeg() {
        assert_eq!(detect_format(b"\x89PNG\r\n\x1a\n"), ImageFormat::Png);
        assert_eq!(detect_format(&[0xff, 0xd8, 0xff, 0xe0]), ImageFormat::Jpeg);
        assert_eq!(detect_format(b"garbage"), ImageFormat::Png);
    }
}

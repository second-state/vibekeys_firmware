//! MQTT 传输:对接 vibetty 的 MQTT 桥接协议。
//!
//! 内部用 esp-idf-svc 的**回调式**客户端 `EspMqttClient::new_cb`:MQTT 事件在 broker 的
//! 内部 task 上以回调投递,回调里把事件数据拷成 owned 后丢进一条 tokio channel,
//! `recv()` 在 app_fut 上从 channel 收。这样彻底绕开旧 async 客户端那套「`conn.next()`
//! 必须与命令并发排水、否则 backpressure 冻死 broker task」的硬性要求 —— 命令
//! (subscribe/publish/unsubscribe)现在是同步调用,不需要 pump,也不会死锁。
//!
//! 协议契约见 `vibetty/docs/esp32-mqtt-integration.md`。实例前缀
//! `P = {user}/{device}/{pid}/vibetty`,其中 device(PC 机器指纹)与 pid(每次重启变)
//! ESP32 都无法预知,必须先通过 presence 发现。

use std::collections::HashMap;
use std::time::Duration;

use embedded_svc::mqtt::client::{Details, EventPayload, QoS};
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use tokio::sync::mpsc;

use crate::protocol::{ClientMessage, ImageFormat, ScreenImageChunk};

/// discovery topic 在 `MqttServer::new` 动态构造:`{user}/+/+/vibetty`,user = username 或 `root`。

/// 单张 screen 重组上限,超过则丢弃防 OOM。
const REASSEMBLY_MAX: usize = 256 * 1024;

/// 回调(broker 内部 task)→ app_fut 之间传递的事件。
///
/// `EspMqttEvent` 借用 broker 内部缓冲,只在回调内有效;所以回调里先把 topic/data 拷成
/// owned 再 `tx.send`,跨线程 / 跨 `.await` 才安全。用无界 channel:回调里的 `send`
/// 永不阻塞,绝不会反过来冻死 broker task(代价是消费端要记得排水,ASR 期间就在做)。
enum RawEvent {
    Connected,
    Disconnected,
    Received {
        topic: String,
        data: Vec<u8>,
        details: Details,
    },
}

pub struct MqttServer {
    client: EspMqttClient<'static>,
    /// 回调 → app_fut 的事件队列。
    rx: mpsc::UnboundedReceiver<RawEvent>,
    /// 会话注册表:presence 公告得到的 prefix -> 会话元信息。
    sessions: HashMap<String, Session>,
    /// 用户选定的活跃会话(= screen 订阅目标 = input 发送目标)。
    /// 当前无活跃时,首个注册会话自动激活;此后不再自动切换。
    active: Option<String>,
    /// 已落实 screen 订阅的实例 prefix(flush_pending 维护)。
    subscribed_prefix: Option<String>,
    /// screen 分片重组缓冲,key = topic。
    reassembly: HashMap<String, Vec<u8>>,
}

/// 单个 vibetty 会话的注册信息。
struct Session {
    client_id: String,
    ts: u64,
}

/// `recv()` 上报给 app 的事件。
pub enum MqttEvent {
    /// 活跃会话的 screen 组装完成,交给 UI 显示。
    ActiveScreen(ScreenImageChunk),
    /// 会话注册表变化:上线(online=true)/下线 LWT(online=false)。
    Presence { prefix: String, online: bool },
}

/// vibetty presence 公告。
#[derive(serde::Deserialize)]
struct Presence {
    prefix: String,
    client_id: String,
    ts: u64,
}

/// 等首条 `Connected`(带超时)。单独成函数以 `&mut rx` 借用,等完归还 rx 给 `Self`。
async fn wait_first_connected(rx: &mut mpsc::UnboundedReceiver<RawEvent>) {
    loop {
        match rx.recv().await {
            Some(RawEvent::Connected) => return,
            Some(_) => continue,
            None => return,
        }
    }
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
            username: username.as_deref(),
            password: password.as_deref(),
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

        // 回调式客户端:broker 内部 task 每收到事件就回调,回调里把 owned 数据丢进 tokio channel。
        let (tx, mut rx) = mpsc::unbounded_channel::<RawEvent>();
        let mut client = EspMqttClient::new_cb(&broker_url, &conf, move |ev| match ev.payload() {
            EventPayload::Connected(session_present) => {
                log::info!("MQTT connected (session_present={session_present})");
                let _ = tx.send(RawEvent::Connected);
            }
            EventPayload::Disconnected => {
                log::warn!("MQTT disconnected");
                let _ = tx.send(RawEvent::Disconnected);
            }
            EventPayload::Received {
                topic,
                data,
                details,
                ..
            } => {
                let _ = tx.send(RawEvent::Received {
                    topic: topic.unwrap_or("").to_string(),
                    data: data.to_vec(),
                    details,
                });
            }
            EventPayload::Error(e) => log::error!("MQTT event error: {e:?}"),
            other => log::debug!("MQTT event: {other:?}"),
        })
        .map_err(|e| anyhow::anyhow!("EspMqttClient::new_cb failed: {e:?}"))?;

        // 等首个 Connected(超时 30s,保留「broker 连不上快速失败」的反馈;连不上 app 会重启)。
        let connected =
            tokio::time::timeout(Duration::from_secs(30), wait_first_connected(&mut rx)).await;
        if connected.is_err() {
            return Err(anyhow::anyhow!("MQTT connect timeout (30s)"));
        }

        // Discovery:订阅 `{user}/+/+/vibetty`(user = username,匿名时回退 `root`)。
        // retained → 一连上立即收到该 user 下所有现存实例的 presence。
        let user = username
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("root");
        let discovery = format!("{user}/+/+/vibetty");
        log::info!("Subscribing discovery topic: {discovery}");
        client
            .subscribe(&discovery, QoS::AtLeastOnce)
            .map_err(|e| anyhow::anyhow!("subscribe discovery failed: {e:?}"))?;

        Ok(Self {
            client,
            rx,
            sessions: HashMap::new(),
            active: None,
            subscribed_prefix: None,
            reassembly: HashMap::new(),
        })
    }

    /// 把 `active` 落实为 screen 订阅。回调式客户端下 subscribe/unsubscribe 是同步调用,
    /// 不再需要像旧 async 客户端那样并发排水 conn 事件。
    pub async fn flush_pending(&mut self) -> anyhow::Result<()> {
        if self.active == self.subscribed_prefix {
            return Ok(());
        }

        // 先退订旧活跃会话的 screen
        if let Some(old) = self.subscribed_prefix.take() {
            log::info!("Unsubscribing old session screen: {old}");
            let _ = self.client.unsubscribe(&format!("{old}/screen"));
            self.reassembly.clear();
        }

        // 再订阅新活跃会话的 screen
        if let Some(new) = self.active.clone() {
            log::info!("Subscribing session screen: {new}");
            self.client
                .subscribe(&format!("{new}/screen"), QoS::AtMostOnce)
                .map_err(|e| anyhow::anyhow!("subscribe screen failed: {e:?}"))?;
            self.subscribed_prefix = Some(new);
        }

        Ok(())
    }

    /// 阻塞等待下一条需要 app 处理的事件(连接 / 断开在内部消化)。
    /// 返回 `None` 表示事件队列已关闭(client 已销毁)。
    pub async fn recv(&mut self) -> Option<MqttEvent> {
        loop {
            // 从 tokio channel 取一条已拷成 owned 的事件。
            let (topic, data, details) = match self.rx.recv().await? {
                RawEvent::Connected => continue,
                RawEvent::Disconnected => continue,
                RawEvent::Received {
                    topic,
                    data,
                    details,
                } => (topic, data, details),
            };

            // presence:正好 4 段且以 /vibetty 结尾。topic 本身即实例 prefix。
            if topic.matches('/').count() == 3 && topic.ends_with("/vibetty") {
                if data.is_empty() {
                    // LWT:实例下线(空 payload = 删除 retained)
                    log::info!("Session offline (LWT): {topic}");
                    self.sessions.remove(&topic);
                    if self.active.as_deref() == Some(topic.as_str()) {
                        self.active = None; // flush_pending 会退订 screen
                    }
                    return Some(MqttEvent::Presence {
                        prefix: topic,
                        online: false,
                    });
                } else {
                    match serde_json::from_slice::<Presence>(&data) {
                        Ok(p) => {
                            // 注册/刷新:保留已存在的 entry(不干扰进行中的重组),只更新元信息。
                            let s = self.sessions.entry(p.prefix.clone()).or_insert(Session {
                                client_id: p.client_id.clone(),
                                ts: p.ts,
                            });
                            s.client_id = p.client_id;
                            s.ts = p.ts;
                            self.cap_sessions();

                            // 首会话自动活跃;此后不再自动切(由用户通过旋钮弹窗手动切换)。
                            let prefix = p.prefix;
                            if self.active.is_none() {
                                log::info!("Auto-activate first session: {prefix}");
                                self.active = Some(prefix.clone());
                                // 立刻请求首帧。screen 订阅由随后的 flush_pending 落实;
                                // 首帧往返(渲染+编码+网络)远慢于订阅落实,不会丢帧。
                                let _ = self.send(ClientMessage::sync()).await;
                            }
                            return Some(MqttEvent::Presence {
                                prefix,
                                online: true,
                            });
                        }
                        Err(e) => log::warn!("Bad presence JSON: {e}"),
                    }
                }
                continue;
            }

            // screen:只订阅了活跃会话,组装完成的必是活跃帧。
            if topic.ends_with("/screen") {
                if let Some(chunk) = self.reassemble_screen(&topic, &data, details) {
                    return Some(MqttEvent::ActiveScreen(chunk));
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
    ) -> Option<ScreenImageChunk> {
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
                Some(ScreenImageChunk {
                    format: detect_format(&complete),
                    data: complete,
                })
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

    /// 发送按键 / 控制消息。`PtyInput` 走 `{P}/pty_in` raw;其余(Sync/Input/Scroll)
    /// 走 `{P}/control` 的 JSON(serde 邻接标签 `{"type":..,"data":..}` 已与服务端对齐)。
    /// 目标 = 用户选定的活跃会话(与 screen 订阅是否已落实无关)。
    pub async fn send(&mut self, msg: ClientMessage) -> anyhow::Result<()> {
        let prefix = self
            .active
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No active vibetty session"))?;

        match msg {
            ClientMessage::PtyInput(bytes) => {
                log::info!("Sending pty_in {bytes:?} ");
                self.client
                    .publish(
                        &format!("{prefix}/pty_in"),
                        QoS::AtMostOnce,
                        false,
                        &bytes[..],
                    )
                    .map_err(|e| anyhow::anyhow!("publish pty_in failed: {e:?}"))?;
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
                    .map_err(|e| anyhow::anyhow!("publish control failed: {e:?}"))?;
            }
        }
        Ok(())
    }

    /// 当前已知会话列表,供选择器渲染。按 ts 升序(再按 prefix)排序,保证 NEXT 稳定。
    /// 返回 (prefix, 显示标签, 是否活跃)。标签优先 client_id,空则回退 prefix。
    pub fn session_labels(&self) -> Vec<(String, String, bool)> {
        let mut entries: Vec<(&String, &Session)> = self.sessions.iter().collect();
        entries.sort_by(|a, b| a.1.ts.cmp(&b.1.ts).then_with(|| a.0.cmp(b.0)));
        entries
            .into_iter()
            .map(|(prefix, s)| {
                let label = if s.client_id.is_empty() {
                    prefix.clone()
                } else {
                    s.client_id.clone()
                };
                (
                    prefix.clone(),
                    label,
                    self.active.as_deref() == Some(prefix.as_str()),
                )
            })
            .collect()
    }

    /// 用户在弹窗里选定一个会话。仅当该 prefix 已注册才生效;
    /// 实际的 screen 订阅切换由随后的 `flush_pending` 落实。
    pub fn set_active(&mut self, prefix: &str) {
        if self.sessions.contains_key(prefix) {
            self.active = Some(prefix.to_string());
        } else {
            log::warn!("set_active: unknown session {prefix}, ignored");
        }
    }

    /// 注册表上限:超过时丢弃 ts 最旧的会话,防 OOM(注册信息很小,安全兜底)。
    fn cap_sessions(&mut self) {
        const MAX_SESSIONS: usize = 8;
        while self.sessions.len() > MAX_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.ts)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.sessions.remove(&k);
                }
                None => break,
            }
        }
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
    username: Option<String>,
    password: Option<String>,
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
            Some((user, pass)) => (Some(user.to_string()), Some(pass.to_string())),
            None => (Some(u.to_string()), None),
        },
        // 无账号(匿名 URL):username/password 都 None → CONNECT 不带凭证(真匿名)。
        // topic 的 user 段则回退 `root`(见 MqttServer::new 的 discovery 订阅)。
        None => (None, None),
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

    #[test]
    fn parse_mqtt_plain() {
        let i = parse_broker_uri("mqtt://alice:secret@broker.example.com:1883").unwrap();
        assert_eq!(i.broker_url, "mqtt://broker.example.com:1883");
        assert_eq!(i.username.as_deref(), Some("alice"));
        assert_eq!(i.password.as_deref(), Some("secret"));
        assert!(!i.use_tls);
    }

    #[test]
    fn parse_mqtts_default_port() {
        let i = parse_broker_uri("mqtts://bob:p@host.io").unwrap();
        assert_eq!(i.broker_url, "mqtts://host.io:8883");
        assert_eq!(i.username.as_deref(), Some("bob"));
        assert!(i.use_tls);
    }

    #[test]
    fn parse_password_with_special_chars() {
        // 密码里含 '@' 会干扰 rfind('@');这里只验证无 @ 的常见情形
        let i = parse_broker_uri("mqtt://u:p-a-ss@1.2.3.4:1883").unwrap();
        assert_eq!(i.username.as_deref(), Some("u"));
        assert_eq!(i.password.as_deref(), Some("p-a-ss"));
        assert_eq!(i.broker_url, "mqtt://1.2.3.4:1883");
    }

    #[test]
    fn parse_anonymous_is_none() {
        // 无 user:pass@(匿名 URL):username/password 都 None(真匿名 CONNECT)
        let i = parse_broker_uri("mqtt://192.168.1.10:1883").unwrap();
        assert!(i.username.is_none());
        assert!(i.password.is_none());
        assert_eq!(i.broker_url, "mqtt://192.168.1.10:1883");
    }

    #[test]
    fn detect_png_jpeg() {
        assert_eq!(detect_format(b"\x89PNG\r\n\x1a\n"), ImageFormat::Png);
        assert_eq!(detect_format(&[0xff, 0xd8, 0xff, 0xe0]), ImageFormat::Jpeg);
        assert_eq!(detect_format(b"garbage"), ImageFormat::Png);
    }
}

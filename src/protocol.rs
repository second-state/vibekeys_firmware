use std::fmt::Debug;

use serde::{Deserialize, Serialize};

// ========== 客户端 -> 服务器 ==========
//
// 设备 -> 服务端的线路协议,需与 vibetty 的 `protocol.rs` 保持一致。
// - 原始按键走 `{prefix}/pty_in`(raw 字节,不经过这里);
// - 控制类消息走 `{prefix}/control`,payload 是 `ClientMessage` 的 serde JSON,
//   `#[serde(tag = "type", content = "data")]` 邻接标签。

/// 客户端发送的消息
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    /// Sync:客户端声明自己显示区的【像素】尺寸 `width`/`height`,
    /// 服务端按 char cell 尺寸换算成列/行后 resize PTY,并回送整张屏幕。
    #[serde(rename = "sync")]
    Sync { width: u16, height: u16 },

    /// PTY 输入（键盘输入发送到终端）
    #[serde(rename = "pty_in")]
    PtyInput(Vec<u8>),

    /// 请求输入（文本输入）
    #[serde(rename = "input_text")]
    Input(String),

    /// 向上滚动;`rows` 缺省/0 = 滚一整页(= 终端可见行数 − 1)
    #[serde(rename = "scroll_up")]
    ScrollUp {
        #[serde(default)]
        rows: u16,
    },

    /// 向下滚动;同 `ScrollUp`
    #[serde(rename = "scroll_down")]
    ScrollDown {
        #[serde(default)]
        rows: u16,
    },
}

impl Debug for ClientMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientMessage::Sync { width, height } => f
                .debug_struct("Sync")
                .field("width", width)
                .field("height", height)
                .finish(),
            ClientMessage::PtyInput(data) => f
                .debug_tuple("PtyInput")
                .field(&format!("[{} bytes]", data.len()))
                .finish(),
            ClientMessage::Input(text) => f.debug_tuple("Input").field(text).finish(),
            ClientMessage::ScrollUp { rows } => f.debug_tuple("ScrollUp").field(rows).finish(),
            ClientMessage::ScrollDown { rows } => f.debug_tuple("ScrollDown").field(rows).finish(),
        }
    }
}

// ========== 设备本地类型（非线路协议）==========
//
// 入站只有两种 payload:`{prefix}/pty_out` 和 `{prefix}/screen` 都是 raw 字节,
// 设备不反序列化任何 ServerMessage 枚举(服务端那边的 `Screen(Arc<vt100::Screen>)`
// 依赖 vt100,ESP32 端不需要也无法镜像)。下面是设备内部用来承载一帧屏幕图的本地结构。

/// 一帧屏幕图片(设备本地重组后的载体,不走 serde 线路序列化)。
/// 入站 `{prefix}/screen` 投递整张 raw 图片字节,这里把它和按 magic bytes 检测出的
/// 图片格式一起交给 UI 渲染。
#[derive(Debug, Clone)]
pub struct ScreenImageChunk {
    /// 图片格式
    pub format: ImageFormat,

    /// 图片数据
    pub data: Vec<u8>,
}

// ========== 辅助类型 ==========

/// 图片格式
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
}

// ========== 客户端消息构造 / JSON ==========

#[allow(dead_code)]
impl ClientMessage {
    /// 创建 PTY 输入消息
    pub fn pty_input(data: Vec<u8>) -> Self {
        Self::PtyInput(data)
    }

    /// 创建 PTY 输入消息（从字符串）
    pub fn pty_input_str(s: &str) -> Self {
        Self::pty_input(s.as_bytes().to_vec())
    }

    /// 创建文本输入消息
    pub fn input(text: impl Into<String>) -> Self {
        Self::Input(text.into())
    }

    /// 构造一帧 Sync:声明设备显示区像素尺寸。
    #[cfg(not(feature = "max2"))]
    pub fn sync() -> Self {
        Self::Sync {
            width: 288,
            height: 5 * 80,
        }
    }

    #[cfg(feature = "max2")]
    pub fn sync() -> Self {
        Self::Sync {
            width: 320,
            height: 3 * 168,
        }
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 JSON 字符串反序列化
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_client_pty_input_json() {
        let msg = ClientMessage::pty_input_str("hello");
        let json = msg.to_json().unwrap();
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::PtyInput(data) => {
                assert_eq!(String::from_utf8_lossy(&data), "hello");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_input_json() {
        let msg = ClientMessage::input("测试文本");
        let json = msg.to_json().unwrap();
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::Input(text) => {
                assert_eq!(text, "测试文本");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_sync_json() {
        let msg = ClientMessage::Sync {
            width: 320,
            height: 172,
        };
        let json = msg.to_json().unwrap();
        assert_eq!(json, r#"{"type":"sync","data":{"width":320,"height":172}}"#);
        match ClientMessage::from_json(&json).unwrap() {
            ClientMessage::Sync { width, height } => assert_eq!((width, height), (320, 172)),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_scroll_json() {
        // rows=0(滚一整页)正常往返
        let msg = ClientMessage::ScrollUp { rows: 0 };
        let json = msg.to_json().unwrap();
        match ClientMessage::from_json(&json).unwrap() {
            ClientMessage::ScrollUp { rows } => assert_eq!(rows, 0),
            _ => panic!("Wrong message type"),
        }

        // 缺省 rows 也应能反序列化(= 0),与服务端 #[serde(default)] 对齐
        match ClientMessage::from_json(r#"{"type":"scroll_down","data":{}}"#).unwrap() {
            ClientMessage::ScrollDown { rows } => assert_eq!(rows, 0),
            _ => panic!("Wrong message type"),
        }
    }
}

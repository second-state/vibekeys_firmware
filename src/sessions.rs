//! Keyboard 模式的多会话状态表(对接 vibekeys_app 0.3.0 会话事件协议)。
//! 协议见 vibekeys_app 仓库的 docs/session-events.md:DISPLAY 特性收到
//! `{"type":"session","ver":1,"sid":"...","proj":"...","st":"..."}` 单行 JSON。
//!
//! 客户端 hooks 不订阅 SessionEnd,没有可靠的删除消息 → 由本表按
//! `last_active` 超时移除(任何事件都刷新它)。

use std::time::{Duration, Instant};

/// 超时阈值:超过该时长没有任何事件的会话自动移除(协议文档建议 30 分钟)。
const SESSION_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 容量上限,满了移除最久未活跃的(与 MAX_WIFI_CREDS 同风格)。
const MAX_SESSIONS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionStatus {
    /// 正在处理用户输入
    Work,
    /// 即将执行工具
    Tool,
    /// 工具执行完成(含 Codex SubagentStop)
    Post,
    /// 等待用户授权(UI 高亮)
    Perm,
    /// 空闲等待用户输入
    Note,
    /// 本轮回答结束
    Done,
    /// 失败
    Err,
    /// 会话结束(协议保留,当前客户端 hooks 不发送;收到即移除该会话)
    End,
}

impl SessionStatus {
    /// 解析协议里的 `st` 字符串;未知值返回 None(调用方静默丢弃)。
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "work" => Self::Work,
            "tool" => Self::Tool,
            "post" => Self::Post,
            "perm" => Self::Perm,
            "note" => Self::Note,
            "done" => Self::Done,
            "err" => Self::Err,
            "end" => Self::End,
            _ => return None,
        })
    }

    /// agent 正在干活(work/tool/post)→ 白色,对应 remote 会话列表的 working。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Work | Self::Tool | Self::Post)
    }

    /// 卡在等授权 → 高亮(黄)。
    pub fn is_perm(self) -> bool {
        self == Self::Perm
    }
}

pub struct SessionEntry {
    /// session-id 前 8 位短码,upsert 的 key
    pub sid: String,
    /// workspace 路径最后一段(项目名)
    pub proj: String,
    /// 所在窗口 id(tmux/iTerm 等,可选,客户端不给则 None)
    pub win_id: Option<String>,
    /// 宿主机操作系统(可选)
    pub os: Option<String>,
    pub st: SessionStatus,
    last_active: Instant,
}

#[derive(Default)]
pub struct SessionTable {
    entries: Vec<SessionEntry>,
}

impl SessionTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 相同 sid upsert(刷新 last_active + 状态);新 sid 追加到尾部。
    /// 容量满时移除最久未活跃的条目。
    pub fn upsert(
        &mut self,
        sid: &str,
        proj: &str,
        win_id: Option<String>,
        os: Option<String>,
        st: SessionStatus,
    ) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.sid == sid) {
            e.proj = proj.to_string();
            e.win_id = win_id;
            e.os = os;
            e.st = st;
            e.last_active = Instant::now();
            return;
        }
        self.entries.push(SessionEntry {
            sid: sid.to_string(),
            proj: proj.to_string(),
            win_id,
            os,
            st,
            last_active: Instant::now(),
        });
        if self.entries.len() > MAX_SESSIONS {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_active)
                .map(|(i, _)| i);
            if let Some(i) = oldest {
                self.entries.remove(i);
            }
        }
    }

    /// 移除超过 SESSION_TIMEOUT 没有事件的会话(客户端不发 end,超时即视为结束)。
    pub fn remove_expired(&mut self) {
        self.entries
            .retain(|e| e.last_active.elapsed() < SESSION_TIMEOUT);
    }

    /// 收到 `st == "end"` 时显式移除(协议保留路径)。
    pub fn remove(&mut self, sid: &str) {
        self.entries.retain(|e| e.sid != sid);
    }

    pub fn list(&self) -> &[SessionEntry] {
        &self.entries
    }
}

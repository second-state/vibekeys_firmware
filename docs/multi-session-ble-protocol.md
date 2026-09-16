# 多会话 BLE 事件支持(对接 vibekeys_app 0.3.0)

> 写给:负责 vibekeys_firmware 的 agent / 开发者
> 来源:vibekeys_app 0.3.0(dev/0.3.0)新增「多会话状态上报」协议
> 客户端侧协议文档(真相来源):`vibekeys_app/docs/session-events.md`
> 客户端实现参考:`vibekeys_app/src/main.rs` 的 `SessionEvent` / `claude_event` / `codex_event`

## 一、这次改了什么

vibekeys_app 的 hooks 链路不再往键盘 DISPLAY 特性发「格式化后的纯文本」,而是发一个
结构化 JSON 事件,让键盘同时维护**多个 agent 会话**(Claude Code、Codex 并行跑)。

**核心兼容性要求:DISPLAY 特性收到的内容,如果解析不出合法 JSON,必须完全走原有逻辑,
行为与 0.2.0 之前一致。** 旧的 `vibekeys send` / `vibekeys notify`(裸文本)不能受影响。

## 二、线格式(客户端 → 设备)

写入通道**不变**:仍是 KEYBOARD_DISPLAY 特性 `cdaa6472-67a8-4241-93cf-145051608573`,
单次 writeWithoutResponse,典型 ~70 字节。

会话事件 payload(紧凑 JSON,单行):

```json
{"type":"session","ver":1,"sid":"abcd1234","proj":"vibekeys_app","st":"tool","os":"macos","win_id":"49446"}
```

| 字段 | 类型 | 必有 | 说明 |
|---|---|---|---|
| `type` | string | 是 | 固定 `"session"`,用于与纯文本分流 |
| `ver` | u8 | 是 | 协议版本,当前 `1` |
| `sid` | string | 是 | session-id 前 8 位短码,**会话表 upsert 的 key** |
| `proj` | string | 是 | workspace 路径最后一段(项目名) |
| `st` | string | 是 | 状态,见下表 |
| `os` | string | 否 | 宿主机 OS 标签:`macos` / `win` / `linux`(客户端 0.3.0-rc.2 起携带;其他平台省略) |
| `win_id` | string | 否 | hook 进程的父进程 id(ppid,十进制字符串;macOS/Linux 携带) |

事件里**没有自由文本字段**(客户端已删掉 `msg`),不需要处理转义/截断。

### 状态枚举

| st | 含义 |
|---|---|
| `work` | 正在处理用户输入 |
| `tool` | 即将执行工具 |
| `post` | 工具执行完成(含 Codex SubagentStop) |
| `perm` | 等待用户授权 |
| `note` | 空闲等待用户输入 |
| `done` | 本轮回答结束 |
| `err` | 失败 |
| `end` | 会话结束(当前客户端 hooks **不发送**,仅协议保留) |

### 会话生命周期(重要)

- 客户端 hooks **不订阅 SessionEnd**,所以**没有可靠的"会话删除"消息**
- 移除策略:**设备端超时**——每个 `sid` 记录最后活跃时间,任何事件都刷新它,
  超过阈值(建议先取 30 分钟,做成可配置常量)自动从会话表移除
- 相同 `sid` 的事件一律 upsert

## 三、固件侧改法建议

现状(改动前):

- `src/bt_keyboard_mode.rs:1043` — DISPLAY `on_write` 把字节 `from_utf8_lossy` 成
  String,发 `ControllerCommand::DisplayKeyboard(s)`
- `src/main.rs:935` — 处理 `DisplayKeyboard`,调 `ui::render_keyboard_view(..., &text)`
- `serde` / `serde_json` 已在 `Cargo.toml` 依赖里,无需新增

### 1. `bt_keyboard_mode.rs` 的 on_write 分流

```rust
display_characteristic.lock().on_write(move |args| {
    let data = args.recv_data();
    let s = String::from_utf8_lossy(&data).to_string();

    // 尝试按会话事件解析;任何失败都退回纯文本旧逻辑
    let session = serde_json::from_str::<serde_json::Value>(&s).ok().and_then(|v| {
        if v.get("type").and_then(|t| t.as_str()) == Some("session") {
            Some((
                v.get("sid").and_then(|x| x.as_str())?.to_string(),
                v.get("proj").and_then(|x| x.as_str())?.to_string(),
                v.get("st").and_then(|x| x.as_str())?.to_string(),
            ))
        } else {
            None
        }
    });

    let result = match session {
        Some((sid, proj, st)) => tx_.blocking_send(ControllerCommand::SessionEvent {
            sid, proj, st,
        }),
        // 不是合法 JSON / 没有 type:"session" → 原有纯文本路径,行为不变
        None => tx_.blocking_send(ControllerCommand::DisplayKeyboard(s)),
    };
    let _ = result;
});
```

要点:

- `serde_json::from_str` 失败(含非 JSON、截断包)= 纯文本,**不 log error**(高频路径)
- JSON 合法但 `type != "session"`(如未来别的类型)→ 同样退回纯文本,保守处理
- `sid`/`proj`/`st` 任一缺失或非字符串 → 视为无效事件,建议静默丢弃(别上屏一坨 JSON)

### 2. 新增 ControllerCommand 变体

`bt_keyboard_mode.rs` 的 `ControllerCommand` enum(约 :1025)加:

```rust
SessionEvent { sid: String, proj: String, st: String },
```

### 3. `main.rs` 事件循环处理(约 :935)

新增一个维护会话表 + 触发重绘的分支。会话表建议直接放 `ui.rs` 或新建
`session.rs`:

```rust
bt_keyboard_mode::ControllerCommand::SessionEvent { sid, proj, st } => {
    sessions::upsert(&sid, &proj, &st);   // 内部刷新 last_active,按 st 更新条目
    sessions::remove_expired();            // 顺手清理超时会话
    let _ = ui::render_session_view(display, wifi_on, &sessions::list());
    // st == "end" 时可直接 remove 该 sid(协议保留,当前客户端不发)
}
```

### 4. 会话表数据结构建议

```rust
struct SessionEntry {
    sid: String,        // 8 字符短码
    proj: String,
    st: SessionStatus,  // work/tool/post/perm/note/done/err/end
    last_active: Instant,
}
```

- 容量上限建议 8 条(与 MAX_WIFI_CREDS 一致的风格),满了移除最旧的
- `st == "perm"` 建议在 UI 上高亮(用户需要知道 agent 卡在等授权)
- 渲染样式由固件侧自行设计;协议不约束每行格式

## 四、验证清单

用装了 0.3.0 dev 分支 CLI 的机器(vibekeys server 已连上设备):

```bash
# 1. 会话事件 → 设备端应 upsert 一条会话,不走纯文本上屏
vibekeys session abcd1234 tool
vibekeys session abcd1234 done

# 2. 同一 sid 再发 → 更新而不是新增条目
vibekeys session abcd1234 perm

# 3. 另一个 sid → 列表出现第二条
vibekeys session 77777777 work

# 4. 纯文本 → 走旧逻辑,整屏显示文本,不影响会话表
vibekeys send "hello"
vibekeys notify "hello"

# 5. 超时移除:把阈值临时调成 30s,等半分钟后确认条目消失
# 6. HTTP 层直接打(验证 400 路径不会发到设备):
curl -X POST http://127.0.0.1:42837/send-json \
  -H 'Content-Type: application/json' -d '{"not":"a session"}'
```

注意第 6 条:该 JSON 能解析但没有 `type:"session"`,按本方案会走纯文本上屏
(`{"not":"a session"}` 显示出来)——这是**预期行为**(保守降级),不是 bug。

## 五、约束与注意

- **不要动 GATT 层**:不加新特性、不改 UUID/属性,协议演进全靠 payload 的 `type` 字段
- 单次 write ≤ MTU(协商后通常 247 字节),当前事件 ~70 字节,不需要分块
- `sid` 只有 8 字符,理论上不同会话可能碰撞(前 8 位相同)——概率极低,可忽略;
  如担心,可把 key 改成 `sid+proj`
- `ver` 字段当前恒为 1;将来格式变更时用它区分,现在解析时忽略即可

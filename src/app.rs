use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use embedded_graphics::prelude::{Dimensions, WebColors};

use crate::{
    bt_keyboard_mode::{self, KeymapConfig},
    lcd::ColorFormat,
    protocol::{self},
};

#[derive(Clone)]
#[allow(dead_code)]
pub enum Event {
    MicAudioChunk(Vec<i16>),
    MicAudioChunkEnd,
    Accept,
    Esc,
    RotateUp,
    RotateDown,
    RotatePush,
    Backspace,
    Custom,
    SwitchMode,
    NEXT,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::MicAudioChunk(_) => write!(f, "MicAudioChunk(...)"),
            Event::MicAudioChunkEnd => write!(f, "MicAudioChunkEnd"),
            Event::Accept => write!(f, "Accept"),
            Event::Esc => write!(f, "Esc"),
            Event::RotateUp => write!(f, "RotateUp"),
            Event::RotateDown => write!(f, "RotateDown"),
            Event::RotatePush => write!(f, "RotatePush"),
            Event::Backspace => write!(f, "Backspace"),
            Event::Custom => write!(f, "Custom"),
            Event::SwitchMode => write!(f, "SwtchMode"),
            Event::NEXT => write!(f, "Next"),
        }
    }
}

enum SelectResult {
    Event(Event),
    Mqtt(crate::mqtt::MqttEvent),
    /// MIC 按键按下(falling edge)。
    MicPressed,
}

/// 同时等待三类事件:`select!` 只负责 select,真正会借用 `server` 的处理放在外层
/// `match` 里 —— 这样各 future 返回后即被释放,避免 `server.recv()` future 与
/// `server.send()` 在同一 `select!` 内的借用冲突。
async fn select_event(
    server: &mut crate::mqtt::MqttServer,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    mic_btn: &mut crate::AnyBtn,
) -> Option<SelectResult> {
    tokio::select! {
        Some(evt) = rx.recv() => Some(SelectResult::Event(evt)),
        Some(msg) = server.recv() => Some(SelectResult::Mqtt(msg)),
        _ = mic_btn.wait_for_low() => Some(SelectResult::MicPressed),
        else => None,
    }
}

pub async fn run(
    uri: String,
    client_id: &str,
    ui: &mut crate::lcd::UI,
    mut rx: crate::audio::EventRx,
    keymaps: &KeymapConfig,
    asr_tx: std::sync::mpsc::Sender<crate::audio::AsrRequest>,
    asr_config: Option<&crate::audio::AsrConfig>,
    mic_btn: &mut crate::AnyBtn,
) -> anyhow::Result<()> {
    log::info!("Connecting to MQTT broker at {uri} with client_id {client_id}");
    let server = crate::mqtt::MqttServer::new(&uri, client_id).await;
    if let Err(e) = &server {
        log::error!("MQTT broker connection failed:\n{e:?}");
        ui.show_notification(ColorFormat::CSS_DARK_RED, "Failed to connect to MQTT")?;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        return Err(anyhow::anyhow!("Failed to connect to MQTT broker"));
    }
    let mut server = server.unwrap();
    // Remote 外壳:连接/首屏前的 stop 占位(收到 vibetty 屏幕后由 ui.handle_message 覆盖)。
    let _ = crate::ui::render_remote_view(ui.display_mut(), false);
    let mut popup = crate::ui::popup_centered(ui.display_mut().bounding_box());
    // ASR 文本编辑器:Some = 正在编辑(屏幕显示编辑器,不刷会话屏);None = 空闲。
    // 用 ui::AsrEditor(ui.rs 弹窗风格),不用 lcd::UI 那套(麦克风状态条,风格不一致)。
    let mut asr_editor: Option<crate::ui::AsrEditor> = None;

    loop {
        // 把 desired_prefix 落实为 subscribe(必须在 select 之外,不可被取消)。
        server.flush_pending().await?;

        let Some(evt) = select_event(&mut server, &mut rx, mic_btn).await else {
            log::warn!("All event sources closed, exiting run loop");
            break;
        };

        // 每轮事件先关闭上一轮弹窗(增量 restore),再处理新事件
        let _ = popup.hide(ui.display_mut());

        match evt {
            SelectResult::Event(e) => match e {
                // 远程模式已改为本地 ASR,音频 chunk 事件不再产生。
                Event::MicAudioChunk(_) | Event::MicAudioChunkEnd => {}
                Event::RotateUp => {
                    if let Some(e) = asr_editor.as_mut() {
                        e.move_left();
                        e.render(ui.display_mut())?;
                    } else {
                        server.send(protocol::ClientMessage::ScrollUp).await?;
                    }
                }
                Event::RotateDown => {
                    if let Some(e) = asr_editor.as_mut() {
                        e.move_right();
                        e.render(ui.display_mut())?;
                    } else {
                        server.send(protocol::ClientMessage::ScrollDown).await?;
                    }
                }
                Event::Esc => {
                    if asr_editor.is_some() {
                        // 放弃编辑,回屏幕。
                        asr_editor = None;
                        let _ = server.send(protocol::ClientMessage::Sync).await;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x1b]))
                            .await?;
                    }
                }
                Event::Accept => {
                    if let Some(mut e) = asr_editor.take() {
                        let input = e.take();
                        let trimmed = input.trim_end();
                        if !trimmed.is_empty() {
                            server
                                .send(protocol::ClientMessage::Input(trimmed.to_string()))
                                .await?;
                        }
                        // 退出编辑后要一帧把屏幕刷回来(编辑期间 ActiveScreen 被排空了)。
                        let _ = server.send(protocol::ClientMessage::Sync).await;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x0d]))
                            .await?;
                    }
                }
                Event::NEXT => {
                    if asr_editor.is_some() {
                        continue; // 编辑 ASR 文本时忽略这些键
                    }
                    if let Some(bytes) = keymaps
                        .keys
                        .get(KeymapConfig::KEY_NEXT)
                        .and_then(|action| key_action_to_ansi(action))
                    {
                        server
                            .send(protocol::ClientMessage::PtyInput(bytes))
                            .await?;
                    } else {
                        // Fallback to default DOWN arrow
                        server
                            .send(protocol::ClientMessage::PtyInput(b"\x1b[B".to_vec()))
                            .await?;
                    }
                }
                Event::Backspace => {
                    if let Some(e) = asr_editor.as_mut() {
                        e.backspace();
                        e.render(ui.display_mut())?;
                    } else {
                        let now = std::time::Instant::now();
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x08]))
                            .await?;
                        log::info!("Backspace sent, took {} ms", now.elapsed().as_millis());
                    }
                }
                Event::SwitchMode => {
                    if asr_editor.is_some() {
                        continue; // 编辑 ASR 文本时忽略这些键
                    }
                    if let Some(bytes) = keymaps
                        .keys
                        .get(KeymapConfig::KEY_SWITCH)
                        .and_then(|action| key_action_to_ansi(action))
                    {
                        server
                            .send(protocol::ClientMessage::PtyInput(bytes))
                            .await?;
                    } else {
                        // Fallback to default DOWN arrow
                        server
                            .send(protocol::ClientMessage::PtyInput(b"\x1b[Z".to_vec()))
                            .await?;
                    }
                }
                Event::RotatePush => {
                    if asr_editor.is_some() {
                        continue; // 编辑 ASR 文本时忽略这些键
                    }
                    // 旋钮按下:不再发按键,改为弹出会话选择器(NEXT 切换 / ACCEPT 确认 / ESC 取消)。
                    open_session_picker(&mut server, ui, &mut rx, &mut popup).await?;
                }
                Event::Custom => {
                    if asr_editor.is_some() {
                        continue; // 编辑 ASR 文本时忽略这些键
                    }
                    if let Some(bytes) = keymaps
                        .keys
                        .get(KeymapConfig::KEY_CUSTOM)
                        .and_then(|action| key_action_to_ansi(action))
                    {
                        server
                            .send(protocol::ClientMessage::PtyInput(bytes))
                            .await?;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(b"/compact".to_vec()))
                            .await?;
                    }
                }
            },
            SelectResult::Mqtt(ev) => match ev {
                crate::mqtt::MqttEvent::ActiveScreen(chunk) => {
                    if asr_editor.is_some() {
                        // 编辑 ASR 文本期间不刷屏,避免覆盖编辑器;只排空保活。
                        log::debug!(
                            "Draining screen chunk ({}B) while in ASR editor",
                            chunk.data.len()
                        );
                    } else if matches!(chunk.format, protocol::ImageFormat::Jpeg) {
                        log::info!("Received screen image: {} bytes (jpeg)", chunk.data.len());
                        crate::ui::display_jpeg(&chunk.data)?;
                    } else {
                        log::warn!("Unsupported screen format: {:?}, only JPEG", chunk.format);
                        let _ = popup.show(ui.display_mut(), "Only JPEG is supported");
                    }
                }
                crate::mqtt::MqttEvent::Presence { prefix, online } => {
                    if online {
                        log::info!("Session registered: {prefix}");
                    } else {
                        log::info!("Session went offline: {prefix}");
                        let _ = popup.show(ui.display_mut(), "session offline");
                    }
                }
            },
            SelectResult::MicPressed => {
                if !mic_btn.is_low() {
                    continue; // 误触发/错误,跳过
                }
                // ASR 跑在独立 OS 线程(见 main.rs 的 asr-worker):Whisper 流式录音 + 网络往返
                // 耗时数十秒,绝不能阻塞本 async 任务(否则 single-thread runtime 冻死 →
                // MQTT conn.next() 不被 poll → backpressure 冻死 broker task →
                // "No PING_RESP, disconnected")。这里只发请求、收结果,select! 里继续排水
                // server.recv() 保活 MQTT,松手时置 cancel 打断录音。
                let cfg = match asr_config {
                    Some(c) => c.clone(),
                    None => {
                        log::warn!("MIC pressed but ASR not configured, ignoring");
                        let _ = mic_btn.wait_for_high().await; // 等松开,避免重复触发
                        continue;
                    }
                };
                let (otx, orx) = tokio::sync::oneshot::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let _ = popup.show(ui.display_mut(), "listening...");
                let req = crate::audio::AsrRequest {
                    config: cfg,
                    cancel: cancel.clone(),
                    respond: otx,
                };
                if asr_tx.send(req).is_err() {
                    // worker 线程没起来 / 已退出。
                    let _ = popup.show(ui.display_mut(), "ASR unavailable");
                    let _ = mic_btn.wait_for_high().await;
                    continue;
                }

                let mut released = false;
                let mut conn_dead = false;
                tokio::pin!(orx);
                let asr_result = loop {
                    tokio::select! {
                        biased;
                        r = &mut orx => break r.unwrap_or_else(|_| {
                            Err(anyhow::anyhow!("ASR worker dropped request"))
                        }),
                        _ = mic_btn.wait_for_high(), if !released => {
                            released = true;
                            cancel.store(true, Ordering::Relaxed);
                        }
                        // 仅排水保活:不更新 UI,避免覆盖 listening 弹窗。
                        // 连接已断时禁用本分支,免得 recv() 持续返回 None 空转。
                        ev = server.recv(), if !conn_dead => {
                            if ev.is_none() {
                                conn_dead = true;
                            }
                        }
                    }
                };

                match asr_result {
                    Ok(text) if !text.trim().is_empty() => {
                        log::info!("Local ASR result: {text}");
                        // 进 ASR 编辑模式:关掉 listening 弹窗,把文本插入光标处,末尾默认补一个空格,
                        // 然后用 ui::AsrEditor(ui.rs 弹窗风格)重绘。
                        let _ = popup.hide(ui.display_mut());
                        let t = text.trim();
                        let e = asr_editor.get_or_insert_with(crate::ui::AsrEditor::new);
                        e.insert_str(&format!("{t} "));
                        e.render(ui.display_mut())?;
                    }
                    Ok(_) => {
                        log::info!("Local ASR returned empty");
                        let _ = popup.show(ui.display_mut(), "(empty)");
                    }
                    Err(e) => {
                        log::error!("Local ASR error: {e:?}");
                        let _ = popup.show(ui.display_mut(), "ASR error");
                    }
                }
            }
        }
    }

    Ok(())
}

/// 会话选择器:旋钮按下时弹出。NEXT 移动焦点、ACCEPT 切换活跃会话、ESC 取消。
/// 只在 `rx` 上等按键;期间 MQTT 消息缓冲在 esp-mqtt 内部队列,短交互无碍。
async fn open_session_picker(
    server: &mut crate::mqtt::MqttServer,
    ui: &mut crate::lcd::UI,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    popup: &mut crate::ui::Popup,
) -> anyhow::Result<()> {
    let labels = server.session_labels();
    if labels.len() <= 1 {
        let _ = popup.show(ui.display_mut(), "no other session");
        return Ok(());
    }
    let items: Vec<String> = labels.iter().map(|(_, label, _)| label.clone()).collect();
    let mut focus = labels
        .iter()
        .position(|(_, _, is_active)| *is_active)
        .unwrap_or(0);

    loop {
        let _ = crate::ui::render_list(ui.display_mut(), "Session (ESC=cancel)", &items, focus);
        match rx.recv().await {
            Some(Event::NEXT) => {
                if !items.is_empty() {
                    focus = (focus + 1) % items.len();
                }
            }
            Some(Event::Accept) => {
                let (prefix, label, _) = &labels[focus];
                server.set_active(prefix);
                // 让新活跃会话立刻推一帧,免得干等下一帧。
                let _ = server.send(protocol::ClientMessage::Sync).await;
                let _ = popup.show(ui.display_mut(), &format!("-> {label}"));
                return Ok(());
            }
            Some(Event::Esc) | None => {
                // 取消:活跃未变。发个 sync 让当前会话立即重绘一帧覆盖列表。
                let _ = server.send(protocol::ClientMessage::Sync).await;
                return Ok(());
            }
            _ => {} // 旋钮方向 / 其它键在选择器里忽略
        }
    }
}

/// Convert KeyAction to ANSI escape sequences for terminal input
///
/// # Arguments
/// * `action` - The key action to convert
///
/// # Returns
/// * `Some(bytes)` - ANSI bytes to send
/// * `None` - Nothing to send (unknown key)
pub fn key_action_to_ansi(action: &bt_keyboard_mode::KeyAction) -> Option<Vec<u8>> {
    use bt_keyboard_mode::KeyAction;

    match action {
        KeyAction::Combo { modifiers, key, .. } => {
            let key_upper = key.to_uppercase();
            let mut result = Vec::new();

            // Check each modifier and apply corresponding ANSI sequence
            let mut has_ctrl = false;
            let mut has_alt = false;
            let mut has_shift = false;

            for mod_name in modifiers {
                match mod_name.as_str() {
                    "ctrl" => has_ctrl = true,
                    "shift" => has_shift = true,
                    "alt" | "option" => has_alt = true,
                    "meta" | "command" | "cmd" | "win" | "gui" => {
                        // Meta not supported in ANSI, ignore
                    }
                    _ => {}
                }
            }

            // Get the base key character
            let base_char = match key_upper.as_str() {
                // Letters
                "A" => Some(b'a'),
                "B" => Some(b'b'),
                "C" => Some(b'c'),
                "D" => Some(b'd'),
                "E" => Some(b'e'),
                "F" => Some(b'f'),
                "G" => Some(b'g'),
                "H" => Some(b'h'),
                "I" => Some(b'i'),
                "J" => Some(b'j'),
                "K" => Some(b'k'),
                "L" => Some(b'l'),
                "M" => Some(b'm'),
                "N" => Some(b'n'),
                "O" => Some(b'o'),
                "P" => Some(b'p'),
                "Q" => Some(b'q'),
                "R" => Some(b'r'),
                "S" => Some(b's'),
                "T" => Some(b't'),
                "U" => Some(b'u'),
                "V" => Some(b'v'),
                "W" => Some(b'w'),
                "X" => Some(b'x'),
                "Y" => Some(b'y'),
                "Z" => Some(b'z'),
                // Numbers
                "0" => Some(b'0'),
                "1" => Some(b'1'),
                "2" => Some(b'2'),
                "3" => Some(b'3'),
                "4" => Some(b'4'),
                "5" => Some(b'5'),
                "6" => Some(b'6'),
                "7" => Some(b'7'),
                "8" => Some(b'8'),
                "9" => Some(b'9'),
                // Special keys
                "SPACE" => Some(b' '),
                "ENTER" | "RETURN" => Some(0x0d),
                "TAB" => Some(0x09),
                "ESC" | "ESCAPE" => Some(0x1b),
                "BACKSPACE" => Some(0x08),
                "DELETE" | "DEL" => Some(0x7f),
                // Arrow keys (ANSI escape sequences)
                "UP" => return Some(b"\x1b[A".to_vec()),
                "DOWN" => return Some(b"\x1b[B".to_vec()),
                "RIGHT" => return Some(b"\x1b[C".to_vec()),
                "LEFT" => return Some(b"\x1b[D".to_vec()),
                // Function keys
                "F1" => return Some(b"\x1bOP".to_vec()),
                "F2" => return Some(b"\x1bOQ".to_vec()),
                "F3" => return Some(b"\x1bOR".to_vec()),
                "F4" => return Some(b"\x1bOS".to_vec()),
                "F5" => return Some(b"\x1b[15~".to_vec()),
                "F6" => return Some(b"\x1b[17~".to_vec()),
                "F7" => return Some(b"\x1b[18~".to_vec()),
                "F8" => return Some(b"\x1b[19~".to_vec()),
                "F9" => return Some(b"\x1b[20~".to_vec()),
                "F10" => return Some(b"\x1b[21~".to_vec()),
                "F11" => return Some(b"\x1b[23~".to_vec()),
                "F12" => return Some(b"\x1b[24~".to_vec()),
                // Symbols (basic set)
                "MINUS" | "-" => Some(b'-'),
                "EQUAL" | "=" => Some(b'='),
                "LEFT_BRACKET" | "[" => Some(b'['),
                "RIGHT_BRACKET" | "]" => Some(b']'),
                "BACKSLASH" | "\\" => Some(b'\\'),
                "SEMICOLON" | ";" => Some(b';'),
                "QUOTE" | "'" => Some(b'\''),
                "COMMA" | "," => Some(b','),
                "PERIOD" | "." => Some(b'.'),
                "SLASH" | "/" => Some(b'/'),
                "GRAVE" | "`" => Some(b'`'),
                _ => {
                    log::warn!("Unknown key for ANSI conversion: {}", key);
                    None
                }
            };

            if let Some(mut ch) = base_char {
                // Apply shift modifier (for letters and symbols)
                if has_shift && ch.is_ascii_lowercase() {
                    ch = ch.to_ascii_uppercase();
                }

                // Handle Ctrl modifier - sends control character (0x00-0x1F)
                // Ctrl+A = 0x01, Ctrl+B = 0x02, ..., Ctrl+Z = 0x1A
                if has_ctrl {
                    if ch.is_ascii_lowercase() || ch.is_ascii_uppercase() {
                        // A-Z maps to 0x01-0x1A
                        ch = ch.to_ascii_uppercase();
                        ch = ch - b'@'; // A=0x41, 0x41-0x40=0x01
                    } else if ch == b' ' {
                        ch = 0x00; // Ctrl+Space = NUL
                    }
                    result.push(ch);
                }

                // Handle Alt modifier - sends ESC prefix + character
                // Alt+T = ESC + t
                if has_alt {
                    result.push(0x1b); // ESC
                    result.push(ch);
                }

                // If no modifiers or only shift, just send the character
                if !has_ctrl && !has_alt {
                    result.push(ch);
                }
            }

            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        }
        KeyAction::Text { value, .. } => Some(value.as_bytes().to_vec()),
    }
}

#[allow(dead_code)]
pub mod key_task {

    pub async fn esc_key(btn: crate::AnyBtn, tx: crate::audio::EventTx) -> anyhow::Result<()> {
        listen_key_event(btn, tx, super::Event::Esc).await
    }

    pub async fn accept_key(btn: crate::AnyBtn, tx: crate::audio::EventTx) -> anyhow::Result<()> {
        listen_key_event(btn, tx, super::Event::Accept).await
    }

    pub async fn rotate_key(
        mut btn_a: crate::AnyBtn,
        btn_b: crate::AnyBtn,
        tx: crate::audio::EventTx,
    ) -> anyhow::Result<()> {
        loop {
            if let Err(_) = btn_a.wait_for_any_edge().await {
                return Err(anyhow::anyhow!("Failed to wait for button edge"));
            }

            if let Err(_) = if btn_a.is_high() {
                if btn_b.is_low() {
                    tx.send(super::Event::RotateDown)
                } else {
                    tx.send(super::Event::RotateUp)
                }
            } else {
                if btn_b.is_low() {
                    tx.send(super::Event::RotateUp)
                } else {
                    tx.send(super::Event::RotateDown)
                }
            }
            .await
            {
                return Err(anyhow::anyhow!("Failed to send rotate event"));
            }
        }
    }

    pub async fn rotate_push_key(
        btn: crate::AnyBtn,
        tx: crate::audio::EventTx,
    ) -> anyhow::Result<()> {
        listen_key_event(btn, tx, super::Event::RotatePush).await
    }

    #[repr(u8)]
    pub enum MicMode {
        PushToTalk,
        Toggle,
    }

    impl Default for MicMode {
        fn default() -> Self {
            Self::Toggle
        }
    }

    impl From<u8> for MicMode {
        fn from(value: u8) -> Self {
            match value {
                0 => Self::PushToTalk,
                1 => Self::Toggle,
                _ => {
                    log::warn!(
                        "Invalid mic mode value: {}, defaulting to PushToTalk",
                        value
                    );
                    Self::PushToTalk
                }
            }
        }
    }

    async fn toggle_mic_key(mut btn: crate::AnyBtn) -> anyhow::Result<()> {
        loop {
            if let Err(e) = btn.wait_for_falling_edge().await {
                log::error!("Button interrupt error: {:?}", e);
                return Err(anyhow::anyhow!("Failed to wait for mic button edge"));
            }

            if !btn.is_low() {
                continue;
            }

            let r = crate::audio::MIC_ON.fetch_not(std::sync::atomic::Ordering::Relaxed);
            log::info!("Button pressed, mic state changed to: {}", !r);

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    async fn push_to_talk_mic_key(mut btn: crate::AnyBtn) -> anyhow::Result<()> {
        loop {
            if let Err(e) = btn.wait_for_any_edge().await {
                log::error!("Button interrupt error: {:?}", e);
                return Err(anyhow::anyhow!("Failed to wait for mic button edge"));
            }

            let is_pressed = btn.is_low();
            crate::audio::MIC_ON.store(is_pressed, std::sync::atomic::Ordering::Relaxed);
            log::info!(
                "Mic button state changed, mic is now {}",
                if is_pressed { "ON" } else { "OFF" }
            );
        }
    }

    pub async fn mic_key(btn: crate::AnyBtn, mode: MicMode) -> anyhow::Result<()> {
        match mode {
            MicMode::PushToTalk => push_to_talk_mic_key(btn).await,
            MicMode::Toggle => toggle_mic_key(btn).await,
        }
    }

    pub async fn backspace_key(
        mut btn: crate::AnyBtn,
        tx: crate::audio::EventTx,
    ) -> anyhow::Result<()> {
        let port = btn.pin();
        loop {
            if let Err(e) = btn.wait_for_falling_edge().await {
                log::error!("Button interrupt error: {:?}", e);
                return Err(anyhow::anyhow!("Failed to wait for button K{port} edge"));
            }
            if !btn.is_low() {
                continue;
            }

            log::info!("Button K{port} pressed");
            if let Err(_) = tx.send(crate::app::Event::Backspace).await {
                return Err(anyhow::anyhow!("Failed to send K{port} event"));
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            if btn.is_low() {
                loop {
                    if !btn.is_low() {
                        break;
                    }

                    log::info!("Button K{port} pressed");
                    if let Err(_) = tx.send(crate::app::Event::Backspace).await {
                        return Err(anyhow::anyhow!("Failed to send K{port} event"));
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }

    pub async fn listen_key_event(
        mut btn: crate::AnyBtn,
        tx: crate::audio::EventTx,
        event: super::Event,
    ) -> anyhow::Result<()> {
        let port = btn.pin();
        loop {
            if let Err(e) = btn.wait_for_falling_edge().await {
                log::error!("Button interrupt error: {:?}", e);
                return Err(anyhow::anyhow!("Failed to wait for button K{port} edge"));
            }

            if !btn.is_low() {
                continue;
            }

            log::info!("Button K{port} pressed");
            if let Err(_) = tx.send(event.clone()).await {
                return Err(anyhow::anyhow!("Failed to send K{port} event"));
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

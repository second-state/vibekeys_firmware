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

/// 发一帧 sync 给活跃会话,按其模式选形态:
/// text 模式 → `sync_cells(cols, rows, close)`(声明终端格子尺寸);
/// JPEG 模式 → `sync_close(close)`(像素尺寸)。`close=true` 暂停服务端主动推屏。
async fn send_active_sync(server: &mut crate::mqtt::MqttServer, close: bool) -> anyhow::Result<()> {
    let msg = if server.active_uses_text_screen() {
        let (cols, rows) = crate::lcd::terminal_text_cells();
        log::info!("Sending text-mode sync: cols={cols} rows={rows} close={close}");
        protocol::ClientMessage::sync_cells(cols, rows, close)
    } else {
        protocol::ClientMessage::sync_close(close)
    };
    server.send(msg).await
}

pub async fn run(
    uri: String,
    client_id: &str,
    ui: &mut crate::lcd::UI,
    mut rx: crate::audio::EventRx,
    keymaps: &KeymapConfig,
    asr_tx: std::sync::mpsc::Sender<crate::audio::AsrRequest>,
    asr_config: Option<&crate::audio::AsrConfig>,
    mic_mode: key_task::MicMode,
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
    // 是否处于「与 broker 断开」状态。断线期间每轮重新 show 下线弹窗(覆盖瞬态提示),
    // 由 Disconnected 置位、Reconnected 清零。首个 Connected 被 wait_first_connected 吃掉,
    // 所以只有真实重连才会触发 Reconnected,不会误关弹窗。
    let mut disconnected = false;

    // 进入 remote 模式直接弹会话列表让用户挑,而不是先自动看活跃会话的屏。
    // 冷启动时 retained presence 还没经 recv() 落进 sessions 表,picker 入口会先等它们到达。
    let _ = open_session_picker(&mut server, ui, &mut rx, &mut popup, false).await;

    // ASR 文本编辑器:Some = 正在编辑(屏幕显示编辑器,不刷会话屏);None = 空闲。
    // 用 ui::AsrEditor(ui.rs 弹窗风格),不用 lcd::UI 那套(麦克风状态条,风格不一致)。
    let mut asr_editor: Option<crate::ui::AsrEditor> = None;

    // 当前屏幕的完整解码帧。滚轮先在这张图上本地平移显示窗口,
    // 平移到头了再通过 MQTT 请服务端继续翻页 —— 避免每一下滚轮都走网络往返。
    let mut current_screen: Option<crate::new_jpg::JpegBufferu16> = None;
    // 显示窗口在 current_screen 中的顶部像素行;0 = 最上面。
    let mut view_window_offset: usize = 0;
    // 当前帧是否还有下文。vibetty 在每帧 JPEG 末尾附一个大端 u32 作为滚动 offset 标记,
    // 0 = 本页是最底页(没有下文)。本地缓冲滚到底后据此决定:有下文才请求下一页,否则忽略。
    let mut current_has_more_below: bool = true;
    // 正在等待服务端的翻页响应。发 ScrollUp/ScrollDown 时置位并显示 loading;新帧
    //(ActiveScreen)到达后据此定位窗口:Up→拉到最底(接旧帧最顶)、Down→拉到最顶(接旧帧最底)。
    #[derive(Clone, Copy)]
    enum PendingScroll {
        Up,
        Down,
    }
    let mut pending_scroll: Option<PendingScroll> = None;
    // pending_scroll 的置位时刻 = 发出翻页请求的 Instant。两用:既做超时检查的起点,
    // 收到响应时又用来算本轮 RTT(更新 rtt_avg)。vibetty 到顶/到底不发图、或丢包时,
    // 超过基于 rtt_avg 算出的阈值就清掉 pending,恢复滚动。
    let mut pending_since: Option<std::time::Instant> = None;
    // 最近翻页往返时间(RTT)的指数移动平均;pending 超时阈值据此自适应。
    let mut rtt_avg: Option<std::time::Duration> = None;
    // 事件循环轮询间隔(也是 pending 超时检查的精度)。
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
    // 超时 = 平均 RTT × 此倍数(留余量给网络抖动)。
    const RTT_TIMEOUT_MULT: u32 = 2;
    // 超时下限,避免 RTT 样本很小时阈值过紧误清。
    const RTT_TIMEOUT_FLOOR: std::time::Duration = std::time::Duration::from_millis(400);
    // 还没有 RTT 样本时(首次翻页前)用的默认超时。
    const RTT_TIMEOUT_DEFAULT: std::time::Duration = std::time::Duration::from_millis(1500);
    let view_windows_height = crate::lcd::DISPLAY_HEIGHT as usize;
    /// 滚轮每格本地平移的像素步长(可调)。
    const SCROLL_STEP_PX: usize = 20;

    loop {
        // 把 desired_prefix 落实为 subscribe(必须在 select 之外,不可被取消)。
        server.flush_pending().await?;

        // 事件获取带轮询超时:无事件时每 POLL_INTERVAL 醒来一次,检查 pending 是否超时
        // (vibetty 到顶/到底不发图 → 翻页请求永远等不到响应,超时清掉才能恢复滚动)。
        let evt =
            match tokio::time::timeout(POLL_INTERVAL, select_event(&mut server, &mut rx, mic_btn))
                .await
            {
                Ok(inner) => inner,
                Err(_) => {
                    // 超时阈值 = 平均 RTT × 倍数(下限 RTT_TIMEOUT_FLOOR);无样本用默认。
                    let timeout = rtt_avg
                        .map(|a| a * RTT_TIMEOUT_MULT)
                        .unwrap_or(RTT_TIMEOUT_DEFAULT)
                        .max(RTT_TIMEOUT_FLOOR);
                    if pending_scroll.is_some()
                        && pending_since.map_or(false, |t| t.elapsed() >= timeout)
                    {
                        log::info!(
                            "Pending scroll timed out (>{timeout:?}, avg RTT={rtt_avg:?}), clearing"
                        );
                        pending_scroll = None;
                        pending_since = None;
                        let _ = popup.hide(ui.display_mut());
                    }
                    continue;
                }
            };
        let Some(evt) = evt else {
            log::warn!("All event sources closed, exiting run loop");
            break;
        };

        // 弹窗收敛:在线则关闭上一轮瞬态弹窗;断线则(重新)显示「下线」弹窗,
        // 让它在无事件期间也持续保持(断线时不会有 MQTT 事件来触发重绘)。
        if disconnected {
            let _ = popup.show(ui.display_mut(), "MQTT disconnected");
        } else {
            let _ = popup.hide(ui.display_mut());
        }

        // 断线期间忽略除 Reconnected 外的一切事件(按键/MIC 发不出去、残余 Presence/Screen
        // 无效),避免这些操作的画面刷新覆盖下线弹窗。首次 Disconnected 由下方 match 立即 show。
        if disconnected && !matches!(evt, SelectResult::Mqtt(crate::mqtt::MqttEvent::Reconnected)) {
            continue;
        }

        match evt {
            SelectResult::Event(e) => match e {
                // 远程模式已改为本地 ASR,音频 chunk 事件不再产生。
                Event::MicAudioChunk(_) | Event::MicAudioChunkEnd => {}
                Event::RotateUp => {
                    if let Some(e) = asr_editor.as_mut() {
                        e.move_left();
                        e.render(ui.display_mut())?;
                    } else if ui.terminal_active() {
                        // text 模式:滚动是本地 scrollback 翻页,无网络往返。
                        let _ = ui.scroll_terminal_text(crate::lcd::TerminalScroll::Up);
                    } else if view_window_offset > 0 {
                        // 还有上方内容:本地平移上去即可,不发 MQTT。
                        view_window_offset = view_window_offset.saturating_sub(SCROLL_STEP_PX);
                        if let Some(buf) = current_screen.as_ref() {
                            if let Err(e) =
                                buf.flush_window(view_window_offset, view_windows_height)
                            {
                                log::error!("Failed to flush screen window: {e:?}");
                            }
                        }
                    } else {
                        // 已在缓冲最顶,请服务端往上翻(scrollback)。不立刻动窗口:保持当前画面、
                        // 显示 loading,等新帧到达再把窗口拉到最底(新帧最底接旧帧最顶,连续阅读不跳)。
                        // loading 期间(pending_scroll 已置位)忽略新的翻页请求,避免连按重复发包/覆盖方向。
                        if pending_scroll.is_none() {
                            pending_scroll = Some(PendingScroll::Up);
                            pending_since = Some(std::time::Instant::now());
                            let _ = popup.show(ui.display_mut(), "loading...");
                            server
                                .send(protocol::ClientMessage::ScrollUp { rows: 0 })
                                .await?;
                        }
                    }
                }
                Event::RotateDown => {
                    if let Some(e) = asr_editor.as_mut() {
                        e.move_right();
                        e.render(ui.display_mut())?;
                    } else if ui.terminal_active() {
                        let _ = ui.scroll_terminal_text(crate::lcd::TerminalScroll::Down);
                    } else {
                        let max_offset = current_screen
                            .as_ref()
                            .map(|b| b.height.saturating_sub(view_windows_height))
                            .unwrap_or(0);
                        if view_window_offset < max_offset {
                            // 还有下方内容:本地平移下去即可,不发 MQTT。
                            view_window_offset =
                                (view_window_offset + SCROLL_STEP_PX).min(max_offset);
                            if let Some(buf) = current_screen.as_ref() {
                                if let Err(e) =
                                    buf.flush_window(view_window_offset, view_windows_height)
                                {
                                    log::error!("Failed to flush screen window: {e:?}");
                                }
                            }
                        } else if current_has_more_below {
                            // 已在缓冲最底、且本页还有下文:请服务端往下翻。不立刻动窗口:保持当前画面、
                            // 显示 loading,等新帧到达再把窗口拉到最顶(新帧最顶接旧帧最底,连续阅读不跳)。
                            // loading 期间(pending_scroll 已置位)忽略新的翻页请求,避免连按重复发包/覆盖方向。
                            if pending_scroll.is_none() {
                                pending_scroll = Some(PendingScroll::Down);
                                pending_since = Some(std::time::Instant::now());
                                let _ = popup.show(ui.display_mut(), "loading...");
                                server
                                    .send(protocol::ClientMessage::ScrollDown { rows: 0 })
                                    .await?;
                            }
                        }
                        // else:本页已是最底(尾部 u32 == 0),没有下文 —— 忽略向下滚动。
                    }
                }
                Event::Esc => {
                    if asr_editor.is_some() {
                        // 放弃编辑,回屏幕。
                        asr_editor = None;
                        // 退出编辑后要一帧把屏幕刷回来(编辑期间屏帧被排空了)。
                        // text 模式发 sync_cells 触发整屏重绘;JPEG 发像素 sync。
                        let _ = send_active_sync(&mut server, false).await;
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
                        // 退出编辑后要一帧把屏幕刷回来(编辑期间屏帧被排空了)。
                        let _ = send_active_sync(&mut server, false).await;
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
                            .send(protocol::ClientMessage::PtyInput(vec![0x7f]))
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
                    open_session_picker(&mut server, ui, &mut rx, &mut popup, true).await?;
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
                        // vibetty 在 JPEG 末尾附了一个大端 u32 作为滚动 offset 标记:
                        // 0 = 本页是最底页(没有下文)。解码前先剥出尾部 4 字节,只把 JPEG 部分喂给解码器。
                        let data = &chunk.data;
                        let (jpeg, has_more_below) = if data.len() >= 4 {
                            let off = data.len() - 4;
                            let marker = u32::from_be_bytes([
                                data[off],
                                data[off + 1],
                                data[off + 2],
                                data[off + 3],
                            ]);
                            (&data[..off], marker != 0)
                        } else {
                            (data.as_slice(), true) // 旧端没附标记:当作还有下文,不阻断向下翻页
                        };
                        log::info!(
                            "Received screen image: {}B jpeg + {}B tail, more_below={}",
                            jpeg.len(),
                            data.len().saturating_sub(jpeg.len()),
                            has_more_below
                        );
                        match crate::new_jpg::esp_jpeg_decode_one_picture(jpeg) {
                            Ok(display) => {
                                // 新帧到达:只有主动翻页(Up/Down)才跳变 offset;服务端定时推送的
                                // screen 保持当前滚动位置不动(flush_window 内部会夹到合法区间,越界也安全)。
                                let max_offset = display.height.saturating_sub(view_windows_height);
                                let taken = pending_scroll.take();
                                let is_response = taken.is_some();
                                match taken {
                                    Some(PendingScroll::Up) => view_window_offset = max_offset,
                                    Some(PendingScroll::Down) => view_window_offset = 0,
                                    None => {}
                                }
                                // 收到翻页响应(is_response):用「发出→收到」时长更新平均 RTT,
                                // 作为后续 pending 超时阈值的依据;定时 screen 则仅清计时。
                                if is_response {
                                    if let Some(sent) = pending_since.take() {
                                        let rtt = sent.elapsed();
                                        rtt_avg = Some(match rtt_avg {
                                            Some(prev) => prev * 3 / 5 + rtt * 2 / 5,
                                            None => rtt,
                                        });
                                        log::debug!("scroll RTT={rtt:?}, avg={rtt_avg:?}");
                                    }
                                } else {
                                    pending_since = None;
                                }
                                if let Err(e) =
                                    display.flush_window(view_window_offset, view_windows_height)
                                {
                                    log::error!("Failed to flush screen window: {e:?}");
                                }
                                current_has_more_below = has_more_below;
                                current_screen = Some(display);
                            }
                            Err(e) => log::error!("Failed to decode JPEG: {e:?}"),
                        }
                    } else {
                        log::warn!("Unsupported screen format: {:?}, only JPEG", chunk.format);
                        let _ = popup.show(ui.display_mut(), "Only JPEG is supported");
                    }
                }
                crate::mqtt::MqttEvent::ActiveText(frame) => {
                    // text 模式屏帧(首字节 tag + ANSI 流)。
                    if asr_editor.is_some() {
                        // 编辑 ASR 文本期间不刷屏(避免覆盖编辑器),只排空保活 MQTT。
                        log::debug!("Draining text frame ({}B) while in ASR editor", frame.len());
                    } else if let Err(e) = ui.show_terminal_text_frame(&frame) {
                        log::error!("flush text screen failed: {e:?}");
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
                crate::mqtt::MqttEvent::Disconnected => {
                    log::warn!("MQTT broker disconnected; showing offline popup");
                    // 断线作废正在等待的翻页请求,避免重连后 resubscribe 的 sync 帧被误当翻页响应。
                    pending_scroll = None;
                    pending_since = None;
                    disconnected = true;
                    let _ = popup.show(ui.display_mut(), "MQTT disconnected");
                }
                crate::mqtt::MqttEvent::Reconnected => {
                    log::info!("MQTT reconnected; subscriptions restored");
                    disconnected = false;
                    let _ = popup.hide(ui.display_mut());
                    // 重连后按活跃会话模式发一帧 sync 要新画面(text→sync_cells,JPEG→像素)。
                    // 无活跃会话时 send 会报错并被忽略。
                    let _ = send_active_sync(&mut server, false).await;
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
                // server.recv() 保活 MQTT,停止时机随 mic_mode(见下方 select!):PTT 松手、
                // Toggle 再按一下,届时置 cancel 打断录音。
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
                tokio::time::sleep(std::time::Duration::from_millis(100)).await; // 防抖
                let asr_result = loop {
                    tokio::select! {
                        biased;
                        r = &mut orx => break r.unwrap_or_else(|_| {
                            Err(anyhow::anyhow!("ASR worker dropped request"))
                        }),
                        // 停止录音的边沿按麦克风模式分:
                        //   PTT  → 松手停止(wait_for_high:此时按下为低,等 rising level 即松手);
                        //   Toggle → 再按一下停止。此时按键仍处于按下(低电平),level 触发的
                        //            wait_for_low 会立刻返回误取消,故用 edge 触发的
                        //            wait_for_falling_edge —— 它要等下一次高→低跳变(松手后再按)。
                        _ = async {
                            match mic_mode {
                                key_task::MicMode::PushToTalk => mic_btn.wait_for_high().await,
                                key_task::MicMode::Toggle => mic_btn.wait_for_falling_edge().await,
                            }
                        }, if !released => {
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

/// 选择器内 select! 产出的事件:只负责取事件,真正借用 server 的处理放在下面
/// 的 `match evt`,避开 select! 内 `server.recv()` 与 `server.send/session_labels` 的借用冲突。
enum PickerEvt {
    Key(Event),
    Mqtt(crate::mqtt::MqttEvent),
    /// 事件源已关闭(rx / server 已销毁)。
    Closed,
}

/// 会话选择器:进入 remote 模式或旋钮按下时打开。NEXT 移动焦点、ACCEPT 切换活跃会话、ESC 取消。
///
/// 打开期间也持续 `server.recv()`:任一会话 presence 变化(状态跳变 / 上下线 / 标题变)
/// 都改变列表内容 → 重新渲染。焦点按 prefix 记,waiting 优先重排时 index 会跳,
/// 按 prefix 记才能跨重排仍指向同一会话。内容没变就不重绘,避免冗余 presence 闪烁。
async fn open_session_picker(
    server: &mut crate::mqtt::MqttServer,
    ui: &mut crate::lcd::UI,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    popup: &mut crate::ui::Popup,
    // `true` = 此刻正在观察活跃屏(中途旋钮按下重开选择器),进入时发 `close=true`
    // 让服务端停推,省得选择器期间推的帧被白白 drain。`false` = 冷启动首次挑选,
    // 还没选定/没在观察任何会话,不该对一个没选过的会话发 close。
    pause_active_push: bool,
) -> anyhow::Result<()> {
    // 入口:retained presence 在 subscribe 后很快到达,但需 poll recv 才会进 sessions 表。
    // 给最多 ENTRY_WAIT 让它们落地(已有 >=2 个会话则立即跳过);期间 ESC 可退出。
    const ENTRY_WAIT_MS: u64 = 1500;
    if server.session_labels().is_empty() {
        let _ =
            crate::ui::render_keyboard_view(ui.display_mut(), false, false, "Loading sessions...");
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(ENTRY_WAIT_MS);
        loop {
            if !server.session_labels().is_empty() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                ev = rx.recv() => match ev {
                    Some(Event::Esc) | None => return Ok(()),
                    _ => {}
                },
                m = server.recv() => { if m.is_none() { return Ok(()); } }
            }
        }
    }

    let mut labels = server.session_labels();
    if labels.is_empty() {
        let _ = popup.show(ui.display_mut(), "no session");
        return Ok(());
    }

    // 焦点按 prefix 记:状态跳变触发 waiting 优先重排,index 会乱跳,故不存 index。
    let mut focus_prefix: String = labels
        .iter()
        .find(|(_, _, is_active, _)| *is_active)
        .or_else(|| labels.first())
        .map(|(p, _, _, _)| p.clone())
        .unwrap();
    // 上次渲染的指纹(items + 焦点行);只在其变化时重绘。
    let mut last_sig: Option<(Vec<(String, bool)>, usize)> = None;

    // 仅当确在观察活跃屏(中途重开)时,进入选择器前发 close=true 让服务端停推——
    // 选择器期间 ActiveScreen 被 drain 排空,推了也是浪费。冷启动首次挑选不带此标志:
    // 此时还没选定任何会话,不该对一个没选过的会话发 close。
    // 所有退出分支都会发 sync()(close=false)恢复推屏并要一帧新画面,进出成对,不泄漏。
    if pause_active_push {
        let _ = send_active_sync(server, true).await;
    }

    loop {
        let items: Vec<(String, bool)> = labels
            .iter()
            .map(|(_, label, _, is_working)| (label.clone(), *is_working))
            .collect();
        let focus_idx = labels
            .iter()
            .position(|(p, _, _, _)| p == &focus_prefix)
            .unwrap_or(0);

        let sig = (items.clone(), focus_idx);
        if last_sig.as_ref() != Some(&sig) {
            let _ = crate::ui::render_session_list(
                ui.display_mut(),
                "Session (ESC=cancel)",
                &items,
                focus_idx,
            );
            last_sig = Some(sig);
        }

        let evt: PickerEvt = tokio::select! {
            ev = rx.recv() => match ev { Some(e) => PickerEvt::Key(e), None => PickerEvt::Closed },
            m = server.recv() => match m { Some(e) => PickerEvt::Mqtt(e), None => PickerEvt::Closed },
        };

        match evt {
            PickerEvt::Closed => {
                let _ = send_active_sync(server, false).await;
                return Ok(());
            }
            PickerEvt::Key(Event::NEXT) => {
                if let Some(i) = labels.iter().position(|(p, _, _, _)| p == &focus_prefix) {
                    let ni = (i + 1) % labels.len();
                    focus_prefix = labels[ni].0.clone();
                }
            }
            PickerEvt::Key(Event::Accept) => {
                server.set_active(&focus_prefix);
                // 切了活跃会话:丢弃旧 text 终端状态(若是 text→JPEG 或换会话),
                // 新会话若是 text 会在首帧 ActiveText 重建。
                ui.clear_terminal();
                // 让新活跃会话立刻推一帧,免得干等下一帧。text 模式发 sync_cells,JPEG 发像素 sync。
                let _ = send_active_sync(server, false).await;
                let label = labels
                    .iter()
                    .find(|(p, _, _, _)| p == &focus_prefix)
                    .map(|(_, l, _, _)| l.clone())
                    .unwrap_or_default();
                let _ = popup.show(ui.display_mut(), &format!("-> {label}"));
                return Ok(());
            }
            PickerEvt::Key(Event::Esc) => {
                // 取消:活跃未变。发个 sync 让当前会话立即重绘一帧覆盖列表。
                let _ = send_active_sync(server, false).await;
                return Ok(());
            }
            PickerEvt::Key(_) => {} // 旋钮方向 / 其它键在选择器里忽略
            PickerEvt::Mqtt(crate::mqtt::MqttEvent::Presence { .. }) => {
                let new_labels = server.session_labels();
                if new_labels.is_empty() {
                    let _ = send_active_sync(server, false).await;
                    let _ = popup.show(ui.display_mut(), "no session");
                    return Ok(());
                }
                labels = new_labels;
                // 焦点会话若已下线,回退到活跃会话或首项。
                if !labels.iter().any(|(p, _, _, _)| p == &focus_prefix) {
                    focus_prefix = labels
                        .iter()
                        .find(|(_, _, a, _)| *a)
                        .or_else(|| labels.first())
                        .map(|(p, _, _, _)| p.clone())
                        .unwrap();
                }
                // 内容变化由下一轮 loop 顶的指纹比较触发重绘。
            }
            PickerEvt::Mqtt(crate::mqtt::MqttEvent::ActiveScreen(_)) => {
                // 选择器开着时不刷屏;退出时 sync() 会要新帧。
            }
            PickerEvt::Mqtt(crate::mqtt::MqttEvent::ActiveText(_)) => {
                // 同上:text 帧在选择器期间也排空,不渲染。
            }
            PickerEvt::Mqtt(crate::mqtt::MqttEvent::Disconnected) => {
                // 选择器内断线:不弹窗(会和列表重绘打架);列表内容随下次 presence 自然更新。
            }
            PickerEvt::Mqtt(crate::mqtt::MqttEvent::Reconnected) => {
                // 选择器内重连:订阅已在 mqtt 层恢复,无需处理。
            }
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
    #[derive(Clone, Copy)]
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

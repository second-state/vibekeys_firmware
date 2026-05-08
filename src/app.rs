use embedded_graphics::prelude::WebColors;

use crate::{
    bt_keyboard_mode::{self, KeymapConfig},
    lcd::{self, ColorFormat},
    protocol::{self},
};

#[derive(Clone)]
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
    ServerMessage(protocol::ServerMessage),
}

async fn select_event(
    server: &mut crate::ws::Server,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
) -> Option<SelectResult> {
    tokio::select! {
        Some(evt) = rx.recv() => {
            Some(SelectResult::Event(evt))
        },
        Some(msg) = server.recv() => {
            Some(SelectResult::ServerMessage(msg))
        },
        else => None,
    }
}

pub async fn run(
    uri: String,
    ui: &mut crate::lcd::UI,
    mut rx: crate::audio::EventRx,
    keymaps: &KeymapConfig,
) -> anyhow::Result<()> {
    let server = crate::ws::Server::new(uri).await;
    if server.is_err() {
        log::error!("Server connection failed:\n{:?}", server.err());
        ui.show_notification(ColorFormat::CSS_DARK_RED, "Failed to connect to server")?;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        return Err(anyhow::anyhow!("Failed to connect to server"));
    }

    let mut server = server.unwrap();
    let mut start_submit_audio = false;

    while let Some(evt) = select_event(&mut server, &mut rx).await {
        match evt {
            SelectResult::Event(e) => match e {
                Event::MicAudioChunk(chunk) => {
                    if !start_submit_audio {
                        start_submit_audio = true;
                        log::info!("Starting to submit audio chunks to server");
                        server
                            .send(protocol::ClientMessage::voice_input_start(Some(16000)))
                            .await?;
                        // ui.show_notification(ColorFormat::CSS_DARK_GREEN, "Voice input started")?;
                        ui.start_input("")?;
                    }
                    let audio_buffer_u8 = unsafe {
                        std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2)
                    };
                    server
                        .send(protocol::ClientMessage::voice_input_chunk(
                            audio_buffer_u8.to_vec(),
                        ))
                        .await?;
                }
                Event::MicAudioChunkEnd => {
                    start_submit_audio = false;
                    server
                        .send(protocol::ClientMessage::voice_input_end())
                        .await?;
                    ui.refresh_input_display()?;
                }
                Event::RotateUp => {
                    if ui.is_input_mode() {
                        ui.move_cursor_left()?;
                    } else {
                        server.send(protocol::ClientMessage::ScrollUp).await?;
                    }
                }
                Event::RotateDown => {
                    if ui.is_input_mode() {
                        ui.move_cursor_right()?;
                    } else {
                        server.send(protocol::ClientMessage::ScrollDown).await?;
                    }
                }
                Event::Esc => {
                    if ui.is_input_mode() {
                        ui.clear_input()?;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x1b]))
                            .await?;
                    }
                }
                Event::Accept => {
                    if ui.is_input_mode() {
                        let input = ui.take_waiting_input_prompt();
                        server.send(protocol::ClientMessage::Input(input)).await?;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x0d]))
                            .await?;
                    }
                }
                Event::NEXT => {
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
                    if ui.is_input_mode() {
                        ui.delete_char_before_cursor()?;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(vec![0x08]))
                            .await?;
                    }
                }
                Event::SwitchMode => {
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
                    if let Some(bytes) = keymaps
                        .keys
                        .get(KeymapConfig::KEY_ROTATE)
                        .and_then(|action| key_action_to_ansi(action))
                    {
                        server
                            .send(protocol::ClientMessage::PtyInput(bytes))
                            .await?;
                    } else {
                        server
                            .send(protocol::ClientMessage::PtyInput(b"/".to_vec()))
                            .await?;
                    }
                }
                Event::Custom => {
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
            SelectResult::ServerMessage(msg) => match msg {
                protocol::ServerMessage::PtyOutput(..) => {
                    log::trace!("Received PTY output, ignoring for now");
                    continue;
                }
                msg => {
                    let ui_msg = lcd::UiMessage::from(msg);
                    ui.handle_message(ui_msg)?;
                }
            },
        }
    }

    Ok(())
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

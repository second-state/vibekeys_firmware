use embedded_graphics::prelude::WebColors;

use crate::{
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

    ui.show_notification(
        ColorFormat::CSS_DARK_GREEN,
        "Server Connected\nPress Voice Key to start talking",
    )?;
    ui.start_input("Ready for input")?;

    while let Some(evt) = select_event(&mut server, &mut rx).await {
        match evt {
            SelectResult::Event(e) => {
                match e {
                    Event::MicAudioChunk(chunk) => {
                        if !start_submit_audio {
                            start_submit_audio = true;
                            log::info!("Starting to submit audio chunks to server");
                            server
                                .send(protocol::ClientMessage::voice_input_start(Some(16000)))
                                .await?;
                            ui.refresh_input_if_waiting()?;
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
                        ui.refresh_input_if_waiting()?;
                        server
                            .send(protocol::ClientMessage::voice_input_end())
                            .await?;
                    }
                    evt => {
                        log::info!("Received event: {:?}", evt);

                        match ui.state() {
                            lcd::UiState::WaitingInput { .. } => {
                                ui.handle_key_event_on_waiting_input(evt, &mut server)
                                    .await?;
                            }
                            lcd::UiState::WaitingChoice { .. } => {
                                ui.handle_key_event_on_choice_selection(evt, &mut server)
                                    .await?;
                            }
                            &lcd::UiState::WaitingChoiceAllowCustom { .. } => {
                                ui.handle_key_event_on_choice_selection(evt, &mut server)
                                    .await?;
                            }
                            lcd::UiState::ShowingNotification { .. } => {
                                ui.handle_key_event_on_displaying_text(evt, &mut server)
                                    .await?;
                            }
                            _ => {
                                log::info!("Received event {:?} in state {:?}, handling with default handler", evt, ui.state());
                            }
                        }
                    }
                }
            }
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

impl lcd::UI {
    async fn handle_key_event_on_waiting_input(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::Esc => {
                self.clear_input()?;
            }
            Event::RotateDown => {
                self.move_cursor_right()?;
            }
            Event::RotateUp => {
                self.move_cursor_left()?;
            }
            Event::Backspace => {
                self.remove_input_char()?;
            }
            Event::Accept => {
                let input = self.get_input().unwrap_or_default();
                if input.is_empty() {
                    log::info!("Input is empty, ignoring submit");
                    return Ok(());
                }
                log::info!("Submitting input: {}", input);
                server.send(protocol::ClientMessage::input(input)).await?;
            }
            Event::SwitchMode => {
                // shift + tab
                server
                    .send(protocol::ClientMessage::PtyInput(b"\x1b[Z".to_vec()))
                    .await?;
            }
            _ => {
                log::warn!("Unexpected event in WaitingInput state");
            }
        }

        Ok(())
    }

    async fn handle_key_event_on_choice_selection(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::RotateDown => {
                if self.allow_input() {
                    self.move_cursor_right()?;
                } else {
                    self.scroll_down()?;
                }
            }
            Event::RotateUp => {
                if self.allow_input() {
                    self.move_cursor_left()?;
                } else {
                    self.scroll_up()?;
                }
            }
            Event::RotatePush => {
                self.reset_scroll()?;
            }
            Event::NEXT => self.next_choice()?,
            Event::Backspace => {
                if self.allow_input() {
                    self.remove_input_char()?;
                }
            }
            Event::Accept => {
                if self.is_confirm_dialog() {
                    server
                        .send(protocol::ClientMessage::pty_input(b"\r".to_vec()))
                        .await?;
                } else {
                    if let Some(choice) = self.confirm_choice() {
                        server.send(choice).await?;
                        log::info!("Confirmed choice, sent to server");
                    } else {
                        log::debug!("No choice selected, ignoring accept event");
                    }
                }
            }
            Event::Esc => {
                if !self.allow_input() {
                    server
                        .send(protocol::ClientMessage::pty_input(b"\x1b".to_vec()))
                        .await?;
                } else {
                    self.clear_input()?;
                }
            }
            _ => {
                log::warn!("Unexpected event in ChoiceSelection state");
            }
        }

        Ok(())
    }

    async fn handle_key_event_on_displaying_text(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::RotateDown => {
                self.scroll_down()?;
            }
            Event::RotateUp => {
                self.scroll_up()?;
            }
            Event::RotatePush => {
                self.reset_scroll()?;
            }
            Event::Accept => {
                self.scroll_up()?;
            }
            Event::SwitchMode => {
                // shift + tab
                server
                    .send(protocol::ClientMessage::PtyInput(b"\x1b[Z".to_vec()))
                    .await?;
            }
            Event::Custom => {
                server.send(protocol::ClientMessage::Sync).await?;
            }
            _ => {
                log::warn!("Unexpected event in DisplayingText state");
            }
        }

        Ok(())
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
            Self::PushToTalk
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

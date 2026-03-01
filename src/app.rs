pub enum Event {
    MicAudioChunk(Vec<i16>),
    MicAudioChunkEnd,
}

pub async fn run(uri: String, mut rx: tokio::sync::mpsc::Receiver<Event>) -> anyhow::Result<()> {
    let mut server = crate::ws::Server::new(uri).await?;
    let mut connected = true;

    while let Some(evt) = rx.recv().await {
        match evt {
            Event::MicAudioChunk(chunk) => {
                if !connected {
                    server.reconnect().await?;
                    connected = true;
                }
                log::info!("Received audio chunk with {} samples", chunk.len());
                let audio_buffer_u8 = unsafe {
                    std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2)
                };

                server
                    .send(tokio_websockets::Message::binary(audio_buffer_u8.to_vec()))
                    .await?;
            }
            Event::MicAudioChunkEnd => {
                // Send an empty message to indicate the end of the chunk
                log::info!("Received audio chunk end");
                server.close().await?;
                connected = false;
            }
        }
    }

    Ok(())
}

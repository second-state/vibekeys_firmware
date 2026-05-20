use std::sync::Arc;

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2s::{config, I2sDriver, I2sRx, I2S0};

use esp_idf_svc::sys::esp_sr;

pub const SAMPLE_RATE: u32 = 16000;

pub static mut AFE_LINEAR_GAIN: f32 = 1.5;
pub static mut AGC_TARGET_LEVEL_DBFS: i32 = 3;
pub static mut AGC_COMPRESSION_GAIN_DB: i32 = 15;

unsafe fn afe_init() -> (
    *mut esp_sr::esp_afe_sr_iface_t,
    *mut esp_sr::esp_afe_sr_data_t,
) {
    let models = std::ptr::null_mut();
    let afe_config = esp_sr::afe_config_init(
        c"M".as_ptr() as _,
        models,
        esp_sr::afe_type_t_AFE_TYPE_VC,
        esp_sr::afe_mode_t_AFE_MODE_HIGH_PERF,
    );
    let afe_config = afe_config.as_mut().unwrap();

    afe_config.pcm_config.sample_rate = 16000;
    afe_config.afe_ringbuf_size = 40;

    afe_config.vad_init = false;
    afe_config.vad_min_noise_ms = 400;
    afe_config.vad_min_speech_ms = 200;
    // afe_config.vad_delay_ms = 250; // Don't change it!!
    afe_config.vad_mode = esp_sr::vad_mode_t_VAD_MODE_4;

    afe_config.agc_init = true;
    afe_config.afe_linear_gain = AFE_LINEAR_GAIN;
    afe_config.agc_target_level_dbfs = AGC_TARGET_LEVEL_DBFS;
    afe_config.agc_compression_gain_db = AGC_COMPRESSION_GAIN_DB;

    afe_config.aec_init = false;
    afe_config.aec_mode = esp_sr::aec_mode_t_AEC_MODE_VOIP_HIGH_PERF;
    // afe_config.aec_filter_length = 5;
    afe_config.ns_init = true;
    afe_config.wakenet_init = false;
    afe_config.memory_alloc_mode = esp_sr::afe_memory_alloc_mode_t_AFE_MEMORY_ALLOC_MORE_PSRAM;

    log::info!("{afe_config:?}");

    let afe_ringbuf_size = afe_config.afe_ringbuf_size;
    log::info!("afe ringbuf size: {}", afe_ringbuf_size);

    let afe_handle = esp_sr::esp_afe_handle_from_config(afe_config);
    let afe_handle = afe_handle.cast_mut().as_mut().unwrap();
    let afe_data = (afe_handle.create_from_config.unwrap())(afe_config);
    let audio_chunksize = (afe_handle.get_feed_chunksize.unwrap())(afe_data);
    log::info!("audio chunksize: {}", audio_chunksize);

    esp_sr::afe_config_free(afe_config);
    (afe_handle, afe_data)
}

struct AFE {
    handle: *mut esp_sr::esp_afe_sr_iface_t,
    data: *mut esp_sr::esp_afe_sr_data_t,
    #[allow(unused)]
    feed_chunksize: usize,
}

unsafe impl Send for AFE {}
unsafe impl Sync for AFE {}

struct AFEResult {
    data: Vec<i16>,
}

impl AFE {
    fn new() -> Self {
        unsafe {
            let (handle, data) = afe_init();
            let feed_chunksize =
                (handle.as_mut().unwrap().get_feed_chunksize.unwrap())(data) as usize;

            AFE {
                handle,
                data,
                feed_chunksize,
            }
        }
    }
    // returns the number of bytes fed

    #[allow(dead_code)]
    fn reset(&self) {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().reset_vad.unwrap())(afe_data);
        }
    }

    #[allow(unused)]
    fn feed(&self, data: &[u8]) -> i32 {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().feed.unwrap())(afe_data, data.as_ptr() as *const i16)
        }
    }

    fn feed_i16(&self, data: &[i16]) -> i32 {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe { (afe_handle.as_ref().unwrap().feed.unwrap())(afe_data, data.as_ptr()) }
    }

    fn fetch_without_cache(&self) -> Result<AFEResult, i32> {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            let result = (afe_handle.as_ref().unwrap().fetch.unwrap())(afe_data)
                .as_mut()
                .unwrap();

            if result.ret_value != 0 {
                return Err(result.ret_value);
            }

            let data_size = result.data_size;

            let mut data = Vec::with_capacity((data_size) as usize / 2);
            if data_size > 0 {
                let data_ = std::slice::from_raw_parts(result.data, data_size as usize / 2);
                data.extend_from_slice(data_);
            }

            Ok(AFEResult { data })
        }
    }
}

pub type EventTx = tokio::sync::mpsc::Sender<crate::app::Event>;
pub type EventRx = tokio::sync::mpsc::Receiver<crate::app::Event>;

pub static MIC_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn afe_worker(afe_handle: Arc<AFE>, tx: EventTx) -> anyhow::Result<()> {
    log::info!("AFE worker started");
    crate::log_heap();
    let mut last_mic_state = false;

    loop {
        let result = afe_handle.fetch_without_cache();
        if let Err(_e) = &result {
            continue;
        }
        let result = result.unwrap();
        if result.data.is_empty() {
            continue;
        }

        let is_mic_on = MIC_ON.load(std::sync::atomic::Ordering::Relaxed);
        if !last_mic_state && is_mic_on {
            log::info!("Mic turned on");
        }

        if is_mic_on {
            tx.blocking_send(crate::app::Event::MicAudioChunk(result.data))
                .map_err(|_| anyhow::anyhow!("Failed to send data"))?;

            last_mic_state = is_mic_on;
            continue;
        }

        if last_mic_state && !is_mic_on {
            log::info!("Mic turned off, resetting AFE VAD state");
            tx.blocking_send(crate::app::Event::MicAudioChunkEnd)
                .map_err(|_| anyhow::anyhow!("Failed to send data"))?;
        }
        last_mic_state = is_mic_on;
    }
}

fn audio_task_run(
    fn_read: &mut dyn FnMut(&mut [i16]) -> Result<usize, esp_idf_svc::sys::EspError>,
    afe_handle: Arc<AFE>,
) -> anyhow::Result<()> {
    let mut conf =
        esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration::get().unwrap_or_default();
    conf.pin_to_core = Some(esp_idf_svc::hal::cpu::Core::Core1);
    let r = conf.set();
    if let Err(e) = r {
        log::error!("Failed to set thread stack alloc caps: {:?}", e);
    }

    let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel::<Vec<i16>>(64);

    let feed_chunksize = afe_handle.feed_chunksize;

    std::thread::Builder::new()
        .name("afe_feed".to_string())
        .stack_size(8 * 1024)
        .spawn(move || {
            log::info!(
                "AFE feed thread started, on core {:?}",
                esp_idf_svc::hal::cpu::core()
            );
            while let Ok(chunk) = chunk_rx.recv() {
                afe_handle.feed_i16(&chunk);
            }
            log::warn!("I2S AFE feed thread exited");
        })?;

    let mut read_buffer = vec![0i16; feed_chunksize];

    loop {
        let len = fn_read(&mut read_buffer)?;

        if len != feed_chunksize * 2 {
            log::warn!(
                "Read size mismatch: expected {}, got {}",
                feed_chunksize * 2,
                len
            );
            break;
        } else {
            chunk_tx.send(read_buffer.clone()).unwrap();
        }
    }

    log::warn!("I2S loop exited");
    Ok(())
}

pub struct AudioWorker {
    pub in_i2s: I2S0<'static>,
    pub in_ws: AnyIOPin<'static>,
    pub in_clk: AnyIOPin<'static>,
    pub din: AnyIOPin<'static>,
    pub in_mclk: Option<AnyIOPin<'static>>,
}

impl AudioWorker {
    pub fn run(self, tx: EventTx) -> anyhow::Result<()> {
        let i2s_config = config::StdConfig::new(
            config::Config::default()
                .auto_clear(true)
                .dma_buffer_count(2)
                .frames_per_buffer(512),
            config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
            config::StdSlotConfig::philips_slot_default(
                config::DataBitWidth::Bits16,
                config::SlotMode::Mono,
            ),
            config::StdGpioConfig::default(),
        );

        let mut rx_driver = I2sDriver::new_std_rx(
            self.in_i2s,
            &i2s_config,
            self.in_clk,
            self.din,
            self.in_mclk,
            self.in_ws,
        )
        .map_err(|e| anyhow::anyhow!("Error create RX: {:?}", e))?;
        rx_driver.rx_enable()?;

        let mut fn_read = |read_buffer: &mut [i16]| -> Result<usize, esp_idf_svc::sys::EspError> {
            let read_buffer_ = unsafe {
                std::slice::from_raw_parts_mut(
                    read_buffer.as_mut_ptr() as *mut u8,
                    read_buffer.len() * std::mem::size_of::<i16>(),
                )
            };

            rx_driver.read(
                read_buffer_,
                esp_idf_svc::hal::delay::TickType::new_millis(50).0,
            )
        };

        let afe_handle = Arc::new(AFE::new());
        let afe_handle_ = afe_handle.clone();

        let _afe_r = std::thread::Builder::new().stack_size(8 * 1024).spawn(|| {
            let r = afe_worker(afe_handle_, tx);
            if let Err(e) = r {
                log::error!("AFE worker error: {:?}", e);
            }
        })?;

        audio_task_run(&mut fn_read, afe_handle)
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "platform")]
pub enum AsrConfig {
    #[serde(alias = "whisper")]
    Whisper {
        uri: String,
        api_key: String,
        model: String,
    },
}

impl AsrConfig {
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let config = serde_json::from_str(json)?;
        Ok(config)
    }

    pub fn load_from_nvs(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> Option<Self> {
        let asr_config_len = nvs.str_len("asr_config").ok()??; // Check if the key exists
        if asr_config_len == 0 {
            return None; // No config stored
        }

        let mut buffer = vec![0u8; asr_config_len];

        let json = nvs.get_str("asr_config", &mut buffer).ok()??;

        Self::from_json(&json).ok()
    }

    pub fn save_to_nvs(&self, nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<()> {
        let json = serde_json::to_string(self)?;
        nvs.set_str("asr_config", &json)?;
        Ok(())
    }

    pub fn requires_tls(&self) -> bool {
        match self {
            AsrConfig::Whisper { uri, .. } => uri.starts_with("https://"),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct AsrResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

impl AsrResult {
    fn parse_text(&self) -> String {
        if self.text.trim().starts_with("[") {
            let mut texts = vec![];
            for line in self.text.lines() {
                if let Some((_, t)) = line.split_once("] ") {
                    texts.push(t.to_string());
                } else {
                    texts.push(line.to_string());
                }
            }
            texts.join("\n")
        } else {
            self.text.clone()
        }
    }
}

pub struct Driver(I2sDriver<'static, I2sRx>);

impl Driver {
    pub fn new(worker: AudioWorker) -> anyhow::Result<Self> {
        let i2s_config = config::StdConfig::new(
            config::Config::default()
                .auto_clear(true)
                .dma_buffer_count(2)
                .frames_per_buffer(512),
            config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
            config::StdSlotConfig::philips_slot_default(
                config::DataBitWidth::Bits16,
                config::SlotMode::Mono,
            ),
            config::StdGpioConfig::default(),
        );

        let mut rx_driver = I2sDriver::new_std_rx(
            worker.in_i2s,
            &i2s_config,
            worker.in_clk,
            worker.din,
            worker.in_mclk,
            worker.in_ws,
        )
        .map_err(|e| anyhow::anyhow!("Error create RX: {:?}", e))?;
        rx_driver.rx_enable()?;

        Ok(Self(rx_driver))
    }

    pub fn read(&mut self, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let len = self
            .0
            .read(buffer, esp_idf_svc::hal::delay::TickType::new_millis(100).0)?;

        Ok(len)
    }

    pub fn start_whisper(
        &mut self,
        uri: &str,
        api_key: &str,
        model: &str,
        mut on_start_listen: impl FnMut(),
        is_stop: impl Fn() -> bool,
    ) -> anyhow::Result<String> {
        let config = esp_idf_svc::http::client::Configuration {
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let conn = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        let mut client = embedded_svc::http::client::Client::wrap(conn);

        // 手动构造 multipart
        let boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
        let content_type = format!("multipart/form-data; boundary={}", boundary);

        let header_value = format!("Bearer {}", api_key);

        let headers = [
            ("Content-Type", content_type.as_str()),
            ("Authorization", header_value.as_str()),
        ];
        let mut req: esp_idf_svc::http::client::Request<
            &mut esp_idf_svc::http::client::EspHttpConnection,
        > = client.post(
            uri,
            if api_key.is_empty() {
                &headers[..1]
            } else {
                &headers
            },
        )?;

        // 写 multipart 头部
        let header = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n",
            boundary
        );
        req.write(header.as_bytes())?;

        let wav_header = crate::util::create_unlimited_wav_header(&crate::util::WavConfig {
            sample_rate: SAMPLE_RATE,
            channels: 1,
            bits_per_sample: 16,
        });
        req.write(&wav_header)?;

        on_start_listen();

        // 边录边写音频数据
        let mut buffer = vec![0u8; 2 * SAMPLE_RATE as usize / 10];
        let max_chunks = 10 * 30; // 30s

        for _ in 0..max_chunks {
            if is_stop() {
                break;
            }
            let len = self.read(&mut buffer)?;
            if len > 0 {
                let n = req.write(&buffer[..len])?;
                log::debug!("Wrote {} bytes of audio data", n);
            }
        }

        // 写 model 字段
        let model_field = format!(
            "\r\n--{}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{}\r\n",
            boundary, model
        );
        req.write(model_field.as_bytes())?;

        // 写结束标记
        let footer = format!("--{}--", boundary);
        req.write(footer.as_bytes())?;
        req.flush()?;
        let mut resp = req.submit()?;
        // buffer.clear();
        log::info!("resp code: {}", resp.status());
        let bytes_read =
            embedded_svc::utils::io::try_read_full(&mut resp, &mut buffer).map_err(|e| e.0)?;
        let resp_body = std::str::from_utf8(&buffer[0..bytes_read])?;
        let asr_result: AsrResult = serde_json::from_str(resp_body)?;
        log::info!("{asr_result:?}");
        if let Some(ref e) = asr_result.error {
            log::error!("error: {}", serde_json::to_string(e).unwrap())
        }

        // let v = serde_json::from_str::<serde_json::Value>(resp_body)?;

        Ok(asr_result.parse_text())
    }

    pub fn start_asr<F: Fn() -> bool, F2: FnMut()>(
        &mut self,
        asr_config: &AsrConfig,
        on_start_listen: F2,
        is_stop: F,
    ) -> anyhow::Result<String> {
        match asr_config {
            AsrConfig::Whisper {
                uri,
                api_key,
                model,
            } => self.start_whisper(uri, api_key, model, on_start_listen, is_stop),
        }
    }
}

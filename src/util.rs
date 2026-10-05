use std::io::Write;

use esp_idf_svc::sys;

/// WAV 音频参数结构体
#[derive(Debug, Clone)]
pub struct WavConfig {
    pub sample_rate: u32,     // 采样率 (Hz)
    pub channels: u16,        // 声道数
    pub bits_per_sample: u16, // 位深度
}

impl Default for WavConfig {
    fn default() -> Self {
        Self {
            sample_rate: crate::audio::SAMPLE_RATE,
            channels: 1,         // 单声道
            bits_per_sample: 16, // 16-bit
        }
    }
}

pub fn create_unlimited_wav_header(config: &WavConfig) -> Vec<u8> {
    let mut wav_data = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut wav_data);

    let bytes_per_sample = config.bits_per_sample / 8;
    let byte_rate = config.sample_rate * config.channels as u32 * bytes_per_sample as u32;
    let block_align = config.channels * bytes_per_sample;
    // let data_size = 0u32;
    // let file_size = 0u32;
    let data_size = 0xFFFFFFFFu32; // unknown data size
    let file_size = 0x7FFFFFFFu32;

    cursor.write_all(b"RIFF").unwrap(); // ChunkID
    cursor.write_all(&file_size.to_le_bytes()).unwrap(); // ChunkSize (little-endian)
    cursor.write_all(b"WAVE").unwrap(); // Format

    // fmt subchunk
    cursor.write_all(b"fmt ").unwrap(); // Subchunk1ID
    cursor.write_all(&16u32.to_le_bytes()).unwrap(); // Subchunk1Size (PCM = 16)
    cursor.write_all(&1u16.to_le_bytes()).unwrap(); // AudioFormat (PCM = 1)
    cursor.write_all(&config.channels.to_le_bytes()).unwrap(); // NumChannels
    cursor.write_all(&config.sample_rate.to_le_bytes()).unwrap(); // SampleRate
    cursor.write_all(&byte_rate.to_le_bytes()).unwrap(); // ByteRate
    cursor.write_all(&block_align.to_le_bytes()).unwrap(); // BlockAlign
    cursor
        .write_all(&config.bits_per_sample.to_le_bytes())
        .unwrap(); // BitsPerSample

    // data subchunk
    cursor.write_all(b"data").unwrap(); // Subchunk2ID
    cursor.write_all(&data_size.to_le_bytes()).unwrap(); // Subchunk2Size

    wav_data
}

// --- PSRAM 栈任务封装 --------------------------------------------------------

/// 栈从 PSRAM 分配的 FreeRTOS 任务(TCB 留内部 RAM)。microlink fork 里
/// ml_create_task_psram() patch 的 Rust 侧对应物。用于 asr-worker 这类
/// 长命线程:16KB 栈从紧缺的内部 RAM 挪到充裕的 PSRAM。
/// 前提:sdkconfig 开 CONFIG_SPIRAM_ALLOW_STACK_EXTERNAL_MEMORY。
pub struct PsramTask {
    handle: sys::TaskHandle_t,
    stack: *mut sys::StackType_t,
    tcb: *mut sys::StaticTask_t,
}

impl PsramTask {
    /// 在 `core` 核上跑 `task_fn(arg)`,栈大小 `stack_size` 以**字节**计
    /// (ESP-IDF 与原版 FreeRTOS 不同,按字节量深度)。
    ///
    /// # Safety
    /// - `task_fn` 与 `arg` 必须在任务整个生命周期内有效。
    /// - drop 前任务必须已停止(栈由 [`Drop`] 释放)。
    pub unsafe fn spawn(
        name: &core::ffi::CStr,
        task_fn: sys::TaskFunction_t,
        arg: *mut core::ffi::c_void,
        stack_size: usize,
        priority: sys::UBaseType_t,
        core: sys::BaseType_t,
    ) -> anyhow::Result<Self> {
        let tcb = sys::heap_caps_malloc(
            core::mem::size_of::<sys::StaticTask_t>(),
            sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT,
        );
        if tcb.is_null() {
            anyhow::bail!("PSRAM task {name:?}: TCB alloc failed");
        }
        let stack =
            sys::heap_caps_malloc(stack_size, sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT);
        if stack.is_null() {
            sys::heap_caps_free(tcb);
            anyhow::bail!("PSRAM task {name:?}: stack alloc failed");
        }
        let handle = sys::xTaskCreateStaticPinnedToCore(
            task_fn,
            name.as_ptr(),
            stack_size as u32,
            arg,
            priority,
            stack.cast(),
            tcb.cast(),
            core,
        );
        if handle.is_null() {
            sys::heap_caps_free(stack);
            sys::heap_caps_free(tcb);
            anyhow::bail!("PSRAM task {name:?}: xTaskCreateStaticPinnedToCore failed");
        }
        log::info!("PSRAM task {name:?} spawned ({} B PSRAM stack)", stack_size);
        Ok(Self {
            handle,
            stack: stack.cast(),
            tcb: tcb.cast(),
        })
    }
}

impl Drop for PsramTask {
    fn drop(&mut self) {
        // SAFETY: 与 spawn 里成功的 heap_caps_malloc 配对。
        unsafe {
            if !self.handle.is_null() {
                sys::vTaskDelete(self.handle);
            }
            sys::heap_caps_free(self.stack.cast());
            sys::heap_caps_free(self.tcb.cast());
        }
    }
}

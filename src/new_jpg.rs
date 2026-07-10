use esp_idf_svc::sys::*;

struct JpegDecoder {
    handle: jpeg_dec_handle_t,
}

impl JpegDecoder {
    fn open(config: &jpeg_dec_config_t) -> Result<Self, i32> {
        unsafe {
            let mut handle: jpeg_dec_handle_t = std::ptr::null_mut();
            let ret = jpeg_dec_open(
                config as *const jpeg_dec_config_t as *mut jpeg_dec_config_t,
                &mut handle,
            );
            if ret != jpeg_error_t_JPEG_ERR_OK {
                return Err(ret);
            }
            Ok(JpegDecoder { handle })
        }
    }
}

impl Drop for JpegDecoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                jpeg_dec_close(self.handle);
            }
        }
    }
}

#[allow(dead_code)]
pub struct JpegBuffer {
    ptr: *mut u8,
    size: usize,
}

#[allow(dead_code)]
impl JpegBuffer {
    fn new(size: usize, aligned: std::ffi::c_int) -> anyhow::Result<Self> {
        unsafe {
            let ptr = jpeg_calloc_align(size, aligned);
            if ptr.is_null() {
                return Err(anyhow::anyhow!("Failed to allocate JPEG buffer"));
            }
            Ok(JpegBuffer {
                ptr: ptr as *mut u8,
                size,
            })
        }
    }

    pub fn flush_to_lcd(&self, w: i32, h: i32) -> i32 {
        let ptr = unsafe { std::slice::from_raw_parts(self.ptr.cast_const(), self.size) };
        // if cfg!(feature = "max2") {
        //     crate::lcd::flush_display(ptr, 0, 0, 320, 168)
        // } else {
        //     crate::lcd::flush_display(ptr, 0, 0, 288, 80)
        // }
        crate::lcd::flush_display(ptr, 0, 0, w, h)
    }
}

impl Drop for JpegBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                jpeg_free_align(self.ptr as *mut _);
            }
        }
    }
}

pub struct JpegBufferu16 {
    pub data: Vec<u128>,
    pub width: usize,
    pub height: usize,
}

impl JpegBufferu16 {
    pub fn new(width: usize, height: usize) -> Self {
        let data = vec![0u128; width * height * 2 / 16]; // 2 bytes per pixel for RGB565

        JpegBufferu16 {
            data,
            width,
            height,
        }
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr() as *mut u8
    }

    pub fn flush_to_lcd(&self) -> anyhow::Result<()> {
        let ptr = unsafe {
            std::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.data.len() * 16)
        };

        let e = crate::lcd::flush_display(ptr, 0, 0, self.width as i32, self.height as i32);
        if e != 0 {
            Err(anyhow::anyhow!("Failed to flush to LCD: error code {}", e))
        } else {
            Ok(())
        }
    }

    pub fn flush_to_lcd_with_offset(
        &self,
        offset_y: usize,
        windows_size: usize,
    ) -> anyhow::Result<()> {
        let ptr = unsafe {
            std::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.data.len() * 16)
        };

        let start = offset_y * self.width * 2; // 2 bytes per pixel for RGB565
        let end = start + windows_size * self.width * 2; // 2 bytes per pixel for RGB565

        let e = crate::lcd::flush_display(
            &ptr[start..end],
            0,
            0,
            self.width as i32,
            windows_size as i32,
        );
        if e != 0 {
            Err(anyhow::anyhow!("Failed to flush to LCD: error code {}", e))
        } else {
            Ok(())
        }
    }

    /// 把缓冲区 `[offset, offset+win_h)` 像素行刷到 LCD(本地滚动用)。
    ///
    /// `offset` 自动夹到合法区间 `[0, height-win_h]`;缓冲比窗口矮时整张刷出
    /// (底部不足的几行保留上一帧残留,与 `flush_to_lcd` 行为一致)。
    pub fn flush_window(&self, offset: usize, win_h: usize) -> anyhow::Result<()> {
        let max_offset = self.height.saturating_sub(win_h);
        let off = offset.min(max_offset);
        let size = (self.height - off).min(win_h);
        self.flush_to_lcd_with_offset(off, size)
    }
}

pub fn esp_jpeg_decode_one_picture(data: &[u8]) -> anyhow::Result<JpegBufferu16> {
    unsafe {
        use esp_idf_svc::sys::*;

        // Generate default configuration
        let mut config = jpeg_dec_config_t::default();
        config.output_type = jpeg_pixel_format_t_JPEG_PIXEL_FORMAT_RGB565_LE;

        if cfg!(feature = "max2") {
            config.scale.height = 168 * 3;
            config.scale.width = 320;
        } else {
            config.scale.height = 80 * 5;
            config.scale.width = 288;
        }

        // Create jpeg_dec handle
        let decoder = JpegDecoder::open(&config)
            .map_err(|e| anyhow::anyhow!("Failed to open JPEG decoder: error code {}", e))?;

        // Create io_callback handle
        let mut jpeg_io = Box::new(jpeg_dec_io_t::default());

        // Create out_info handle
        let mut out_info = Box::new(jpeg_dec_header_info_t::default());

        // Set input buffer and buffer len to io_callback
        jpeg_io.inbuf = data.as_ptr() as *mut u8;
        jpeg_io.inbuf_len = data.len() as i32;

        // Parse jpeg picture header and get picture for user and decoder
        let ret = jpeg_dec_parse_header(decoder.handle, jpeg_io.as_mut(), out_info.as_mut());
        if ret != jpeg_error_t_JPEG_ERR_OK {
            return Err(anyhow::anyhow!(
                "Failed to parse JPEG header: error code {}",
                ret
            ));
        }

        // Calculate output length based on pixel format
        // Default to RGB565 (2 bytes per pixel)
        // let out_len = (*out_info).width as usize * (*out_info).height as usize * 2;

        // Allocate aligned output buffer
        let mut out_buf =
            JpegBufferu16::new((*out_info).width as usize, (*out_info).height as usize);

        jpeg_io.outbuf = out_buf.as_mut_ptr() as *mut u8;

        // Start decode jpeg
        let ret = jpeg_dec_process(decoder.handle, jpeg_io.as_mut());
        if ret != jpeg_error_t_JPEG_ERR_OK {
            return Err(anyhow::anyhow!("Failed to decode JPEG: error code {}", ret));
        }

        Ok(out_buf)
    }
}

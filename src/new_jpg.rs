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

pub struct JpegBuffer {
    ptr: *mut u8,
    size: usize,
}

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

    pub fn flush_to_lcd(&self) -> i32 {
        let ptr = unsafe { std::slice::from_raw_parts(self.ptr.cast_const(), self.size) };
        if cfg!(feature = "max2") {
            crate::lcd::flush_display(ptr, 0, 0, 320, 168)
        } else {
            crate::lcd::flush_display(ptr, 0, 0, 288, 80)
        }
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

pub fn esp_jpeg_decode_one_picture(data: &[u8]) -> anyhow::Result<JpegBuffer> {
    unsafe {
        use esp_idf_svc::sys::*;

        // Generate default configuration
        let mut config = jpeg_dec_config_t::default();
        config.output_type = jpeg_pixel_format_t_JPEG_PIXEL_FORMAT_RGB565_LE;

        if cfg!(feature = "max2") {
            config.clipper.height = 168;
            config.clipper.width = 320;
        } else {
            config.clipper.height = 80;
            config.clipper.width = 288;
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
        let out_len = (*out_info).width as usize * (*out_info).height as usize * 2;

        // Allocate aligned output buffer
        let out_buf = JpegBuffer::new(out_len, 16)?;

        jpeg_io.outbuf = out_buf.ptr;

        // Start decode jpeg
        let ret = jpeg_dec_process(decoder.handle, jpeg_io.as_mut());
        if ret != jpeg_error_t_JPEG_ERR_OK {
            return Err(anyhow::anyhow!("Failed to decode JPEG: error code {}", ret));
        }

        Ok(out_buf)
    }
}

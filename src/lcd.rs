use std::{collections::HashSet, fmt::Debug};

use embedded_graphics::{
    framebuffer::{buffer_size, Framebuffer},
    image::GetPixel,
    pixelcolor::{
        raw::{LittleEndian, RawU16},
        Rgb565,
    },
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, StyledDrawable},
    Pixel,
};
use esp_idf_svc::{
    hal::{
        gpio::{Gpio12, Gpio13, Gpio14, Gpio21, Gpio47, Pin},
        spi::SPI3,
    },
    sys::EspError,
};
use u8g2_fonts::U8g2TextStyle;

use crate::protocol::ClientMessage;

pub const DISPLAY_WIDTH: usize = 284;
pub const DISPLAY_HEIGHT: usize = 78;
static mut ESP_LCD_PANEL_HANDLE: esp_idf_svc::sys::esp_lcd_panel_handle_t = std::ptr::null_mut();
pub type ColorFormat = Rgb565;

pub fn init_spi(_spi: SPI3, mosi: Gpio21, clk: Gpio47) -> Result<(), EspError> {
    use esp_idf_svc::hal::spi::Spi;
    use esp_idf_svc::sys::*;
    const GPIO_NUM_NC: i32 = -1;

    let mut buscfg = spi_bus_config_t::default();
    buscfg.__bindgen_anon_1.mosi_io_num = mosi.pin();
    buscfg.__bindgen_anon_2.miso_io_num = GPIO_NUM_NC;
    buscfg.sclk_io_num = clk.pin();
    buscfg.__bindgen_anon_3.quadwp_io_num = GPIO_NUM_NC;
    buscfg.__bindgen_anon_4.quadhd_io_num = GPIO_NUM_NC;
    buscfg.max_transfer_sz = (DISPLAY_WIDTH * DISPLAY_HEIGHT * std::mem::size_of::<u16>()) as i32;
    esp!(unsafe { spi_bus_initialize(SPI3::device(), &buscfg, spi_common_dma_t_SPI_DMA_CH_AUTO,) })
}

pub fn init_lcd(cs: Gpio12, dc: Gpio13, rst: Gpio14) -> Result<(), EspError> {
    use esp_idf_svc::sys::*;

    ::log::info!("Install panel IO");
    let mut panel_io: esp_lcd_panel_io_handle_t = std::ptr::null_mut();
    let mut io_config = esp_lcd_panel_io_spi_config_t::default();
    io_config.cs_gpio_num = cs.pin();
    io_config.dc_gpio_num = dc.pin();
    io_config.spi_mode = 3;
    io_config.pclk_hz = 40 * 1000 * 1000;
    io_config.trans_queue_depth = 10;
    io_config.lcd_cmd_bits = 8;
    io_config.lcd_param_bits = 8;
    esp!(unsafe {
        esp_lcd_new_panel_io_spi(spi_host_device_t_SPI3_HOST as _, &io_config, &mut panel_io)
    })?;

    ::log::info!("Install LCD driver");

    let mut panel_config = esp_lcd_panel_dev_config_t::default();
    let mut panel: esp_lcd_panel_handle_t = std::ptr::null_mut();

    panel_config.reset_gpio_num = rst.pin();
    panel_config.data_endian = lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE;
    panel_config.__bindgen_anon_1.rgb_ele_order = lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_RGB;
    panel_config.bits_per_pixel = 16;

    esp!(unsafe { esp_lcd_new_panel_st7789(panel_io, &panel_config, &mut panel) })?;

    unsafe {
        ESP_LCD_PANEL_HANDLE = panel;
    }

    const DISPLAY_MIRROR_X: bool = true;
    const DISPLAY_MIRROR_Y: bool = false;
    const DISPLAY_SWAP_XY: bool = true;
    const DISPLAY_INVERT_COLOR: bool = false;

    ::log::info!("Reset LCD panel");
    unsafe {
        esp!(esp_lcd_panel_set_gap(panel, 18, 82))?;
        esp!(esp_lcd_panel_reset(panel))?;
        esp!(esp_lcd_panel_init(panel))?;
        esp!(esp_lcd_panel_invert_color(panel, DISPLAY_INVERT_COLOR))?;
        esp!(esp_lcd_panel_swap_xy(panel, DISPLAY_SWAP_XY))?;
        esp!(esp_lcd_panel_mirror(
            panel,
            DISPLAY_MIRROR_X,
            DISPLAY_MIRROR_Y
        ))?;
        esp!(esp_lcd_panel_disp_on_off(panel, true))?; /* 启动屏幕 */
    }

    Ok(())
}

pub fn flush_display(color_data: &[u8], x_start: i32, y_start: i32, x_end: i32, y_end: i32) -> i32 {
    unsafe {
        let e = esp_idf_svc::sys::esp_lcd_panel_draw_bitmap(
            ESP_LCD_PANEL_HANDLE,
            x_start,
            y_start,
            x_end,
            y_end,
            color_data.as_ptr().cast(),
        );
        if e != 0 {
            log::warn!("flush_display error: {}", e);
        }
        e
    }
}

/*
const LEDC_MAX_DUTY: u32 = (1 << 13) - 1;
pub fn set_backlight<'d>(
    ledc_driver: &mut esp_idf_svc::hal::ledc::LedcDriver<'d>,
    light: u8,
) -> anyhow::Result<()> {
    let light = 100.min(light) as u32;
    let duty = LEDC_MAX_DUTY - (81 * (100 - light));
    let duty = if light == 0 { 0 } else { duty };
    ledc_driver.set_duty(duty)?;
    Ok(())
}

pub fn backlight_init(
    bl_pin: esp_idf_svc::hal::gpio::AnyIOPin,
) -> anyhow::Result<esp_idf_svc::hal::ledc::LedcDriver<'static>> {
    use esp_idf_svc::hal;
    let config = hal::ledc::config::TimerConfig::new()
        .resolution(hal::ledc::Resolution::Bits13)
        .frequency(hal::units::Hertz(6400));
    let time = unsafe { hal::ledc::TIMER0::new() };
    let timer_driver = hal::ledc::LedcTimerDriver::new(time, &config)?;

    let ledc_driver =
        hal::ledc::LedcDriver::new(unsafe { hal::ledc::CHANNEL0::new() }, timer_driver, bl_pin)?;

    Ok(ledc_driver)
}

*/

#[derive(Debug, Clone)]
pub struct MyTextStyle {
    pub font_style: U8g2TextStyle<ColorFormat>,
    pub vertical_offset: i32,
    pub bg_color: Option<ColorFormat>,
}

impl embedded_graphics::text::renderer::TextRenderer for MyTextStyle {
    type Color = ColorFormat;

    fn draw_string<D>(
        &self,
        text: &str,
        mut position: Point,
        baseline: embedded_graphics::text::Baseline,
        target: &mut D,
    ) -> Result<Point, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        position.y += self.vertical_offset;

        if let Some(bg) = self.bg_color {
            let text_metrics = self.font_style.measure_string(text, position, baseline);
            Rectangle::new(
                position,
                Size::new(text_metrics.bounding_box.size.width + 1, self.line_height()),
            )
            .draw_styled(&PrimitiveStyle::with_fill(bg), target)?;
        }

        self.font_style
            .draw_string(text, position, baseline, target)
    }

    fn draw_whitespace<D>(
        &self,
        width: u32,
        mut position: Point,
        baseline: embedded_graphics::text::Baseline,
        target: &mut D,
    ) -> Result<Point, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        position.y += self.vertical_offset;
        if let Some(bg) = self.bg_color {
            Rectangle::new(position, Size::new(width, self.line_height()))
                .draw_styled(&PrimitiveStyle::with_fill(bg), target)?;
        }
        self.font_style
            .draw_whitespace(width, position, baseline, target)
    }

    fn measure_string(
        &self,
        text: &str,
        mut position: Point,
        baseline: embedded_graphics::text::Baseline,
    ) -> embedded_graphics::text::renderer::TextMetrics {
        position.y += self.vertical_offset;
        self.font_style.measure_string(text, position, baseline)
    }

    fn line_height(&self) -> u32 {
        self.font_style.line_height()
    }
}

impl embedded_graphics::text::renderer::CharacterStyle for MyTextStyle {
    type Color = ColorFormat;

    fn set_text_color(&mut self, text_color: Option<Self::Color>) {
        self.font_style
            .set_text_color(Some(text_color.unwrap_or(ColorFormat::CSS_BLACK)));
    }

    fn set_background_color(&mut self, background_color: Option<Self::Color>) {
        self.bg_color = background_color;
    }

    fn set_underline_color(
        &mut self,
        underline_color: embedded_graphics::text::DecorationColor<Self::Color>,
    ) {
        self.font_style.set_underline_color(underline_color);
    }

    fn set_strikethrough_color(
        &mut self,
        strikethrough_color: embedded_graphics::text::DecorationColor<Self::Color>,
    ) {
        self.font_style.set_strikethrough_color(strikethrough_color);
    }
}

pub trait DisplayTargetDrive:
    DrawTarget<Color = ColorFormat> + GetPixel<Color = ColorFormat>
{
    fn new(color: ColorFormat) -> Self;
    fn fill_color(&mut self, color: ColorFormat) -> anyhow::Result<()>;
    fn flush(&mut self) -> anyhow::Result<()>;
    fn fix_background(&mut self) -> anyhow::Result<()>;
}

type Framebuffer_ = Framebuffer<
    ColorFormat,
    RawU16,
    LittleEndian,
    DISPLAY_WIDTH,
    DISPLAY_HEIGHT,
    { buffer_size::<ColorFormat>(DISPLAY_WIDTH, DISPLAY_HEIGHT) },
>;

pub struct FrameBuffer {
    buffers: Box<Framebuffer_>,
    background_buffers: Box<Framebuffer_>,
}

impl Dimensions for FrameBuffer {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(
            Point::new(0, 0),
            Size::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32),
        )
    }
}

impl DrawTarget for FrameBuffer {
    type Color = ColorFormat;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
    {
        self.buffers.draw_iter(pixels)?;
        Ok(())
    }
}

impl GetPixel for FrameBuffer {
    type Color = ColorFormat;

    fn pixel(&self, point: Point) -> Option<Self::Color> {
        self.buffers.pixel(point)
    }
}

impl DisplayTargetDrive for FrameBuffer {
    fn new(color: ColorFormat) -> Self {
        let mut s = Self {
            buffers: Box::new(Framebuffer::new()),
            background_buffers: Box::new(Framebuffer::new()),
        };

        s.buffers.clear(color).unwrap();
        s.background_buffers.clear(color).unwrap();

        s
    }

    fn fill_color(&mut self, color: ColorFormat) -> anyhow::Result<()> {
        self.buffers.clear(color)?;
        self.background_buffers.clear(color)?;
        Ok(())
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let bounding_box = self.bounding_box();
        let x_start = bounding_box.top_left.x as i32;
        let y_start = bounding_box.top_left.y as i32;
        let x_end = bounding_box.top_left.x + bounding_box.size.width as i32;
        let y_end = bounding_box.top_left.y + bounding_box.size.height as i32;

        let e = flush_display(self.buffers.data(), x_start, y_start, x_end, y_end);
        if e != 0 {
            return Err(anyhow::anyhow!("Failed to flush display: error code {}", e));
        }

        self.buffers.clone_from(&self.background_buffers);

        Ok(())
    }

    fn fix_background(&mut self) -> anyhow::Result<()> {
        self.background_buffers.clone_from(&self.buffers);
        Ok(())
    }
}

pub const DEFAULT_BACKGROUND: &[u8] = &[];

pub fn display_png<D: DisplayTargetDrive>(
    display_target: &mut D,
    png: &[u8],
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let img_reader =
        image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);

    let img = img_reader.decode().unwrap().to_rgb8();

    let p = img.enumerate_pixels().map(|(x, y, p)| {
        Pixel(
            Point::new(x as i32, y as i32),
            ColorFormat::new(
                p[0] / (u8::MAX / ColorFormat::MAX_R),
                p[1] / (u8::MAX / ColorFormat::MAX_G),
                p[2] / (u8::MAX / ColorFormat::MAX_B),
            ),
        )
    });

    display_target
        .draw_iter(p)
        .map_err(|_| anyhow::anyhow!("Failed to draw PNG image"))?;

    display_target.fix_background()?;

    display_target.flush()?;

    std::thread::sleep(timeout);

    Ok(())
}

mod new_jpg {
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
            super::flush_display(ptr, 0, 0, 288, 80)
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
            config.clipper.height = 80;
            config.clipper.width = 288;

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
            let out_len = (*out_info).width * (*out_info).height * 2;

            // Allocate aligned output buffer
            let out_buf = JpegBuffer::new(out_len as usize, 16)?;

            jpeg_io.outbuf = out_buf.ptr;

            // Start decode jpeg
            let ret = jpeg_dec_process(decoder.handle, jpeg_io.as_mut());
            if ret != jpeg_error_t_JPEG_ERR_OK {
                return Err(anyhow::anyhow!("Failed to decode JPEG: error code {}", ret));
            }

            Ok(out_buf)
        }
    }
}

pub fn display_jpeg(jpeg: &[u8]) -> anyhow::Result<()> {
    let jpeg_buffer = new_jpg::esp_jpeg_decode_one_picture(jpeg)?;
    let e = jpeg_buffer.flush_to_lcd();
    if e != 0 {
        return Err(anyhow::anyhow!(
            "Failed to flush JPEG to LCD: error code {}",
            e
        ));
    }
    Ok(())
}

pub fn display_text(
    display_target: &mut FrameBuffer,
    text: &str,
    scroll_offset: i32,
) -> anyhow::Result<()> {
    let area_box = display_target.bounding_box();

    let textbox_style = embedded_text::style::TextBoxStyleBuilder::new()
        .height_mode(embedded_text::style::HeightMode::ShrinkToText(
            embedded_text::style::VerticalOverdraw::FullRowsOnly,
        ))
        .alignment(embedded_text::alignment::HorizontalAlignment::Center)
        .line_height(embedded_graphics::text::LineHeight::Pixels(14))
        .build();

    embedded_text::TextBox::with_textbox_style(
        text,
        area_box,
        MyTextStyle {
            font_style: U8g2TextStyle::new(
                u8g2_fonts::fonts::u8g2_font_wqy12_t_gb2312,
                ColorFormat::CSS_WHEAT,
            ),
            vertical_offset: 3,
            bg_color: None,
        },
        textbox_style,
    )
    .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
    .set_vertical_offset(scroll_offset)
    .draw(display_target)?;

    // display_target.fix_background()?;

    display_target.flush()?;

    Ok(())
}

// ========== UI 消息类型 (对应 ServerMessage) ==========

/// UI 渲染消息类型 (对应 protocol.rs 中的 ServerMessage)
#[derive(Clone)]
pub enum UiMessage {
    /// 屏幕显示图片
    ScreenImage {
        data: Vec<u8>,
        format: ImageFormat,
        is_last: bool,
    },

    /// 通知消息
    Notification {
        color: ColorFormat,
        message: String,
        title: Option<String>,
    },

    /// ASR 结果
    AsrResult(String),
}

impl Debug for UiMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiMessage::ScreenImage {
                data,
                format,
                is_last,
            } => f
                .debug_struct("ScreenImage")
                .field("format", format)
                .field("data_len", &data.len())
                .field("is_last", is_last)
                .finish(),
            UiMessage::Notification {
                message,
                title,
                color,
            } => f
                .debug_struct("Notification")
                .field("color", color)
                .field("message", &message.chars().take(20).collect::<String>())
                .field("title", title)
                .finish(),
            UiMessage::AsrResult(text) => f
                .debug_tuple("AsrResult")
                .field(&text.chars().take(20).collect::<String>())
                .finish(),
        }
    }
}

/// 图片格式 (对应 protocol.rs)
#[derive(Clone, Copy, Debug)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
}

/// 通知级别 (对应 protocol.rs)
#[derive(Clone, Copy, Debug)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl NotificationLevel {
    /// 获取对应的颜色
    pub fn to_color(self) -> ColorFormat {
        match self {
            NotificationLevel::Info => ColorFormat::new(0, 100, 255), // 蓝色
            NotificationLevel::Success => ColorFormat::new(0, 200, 255), // 青色
            NotificationLevel::Warning => ColorFormat::new(255, 150, 0), // 橙色
            NotificationLevel::Error => ColorFormat::new(255, 0, 0),  // 红色
        }
    }
}

// ========== UI 状态 ==========

// ========== UI 组件 ==========

/// UI 渲染配置
#[derive(Clone, Debug)]
pub struct UiConfig {
    /// 字体颜色
    pub text_color: ColorFormat,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            text_color: ColorFormat::CSS_WHEAT,
        }
    }
}

// ========== UI 主结构 ==========

/// UI 管理器
///
/// 负责管理 LCD 显示和用户交互，对应 protocol.rs 中的消息设计
pub struct UI {
    /// 显示缓冲区
    display: FrameBuffer,

    /// UI 配置
    config: UiConfig,

    asr_input: String,
    asr_cursor_pos: usize,
    input_mode: bool,

    image_buffer: Vec<u8>,
}

impl UI {
    /// 创建新的 UI 实例
    pub fn new() -> Self {
        Self {
            display: FrameBuffer::new(ColorFormat::new(30, 30, 30)),
            input_mode: false,
            image_buffer: Vec::with_capacity(1024),
            config: UiConfig::default(),
            asr_input: String::new(),
            asr_cursor_pos: 0,
        }
    }

    /// 使用指定显示目标创建 UI
    pub fn new_with_target(display: FrameBuffer) -> Self {
        Self {
            display,
            input_mode: false,
            image_buffer: Vec::with_capacity(1024),
            config: UiConfig::default(),
            asr_input: String::new(),
            asr_cursor_pos: 0,
        }
    }

    /// 处理 UI 消息 (对应 protocol.rs 的 ServerMessage)
    pub fn handle_message(&mut self, msg: UiMessage) -> anyhow::Result<()> {
        match msg {
            UiMessage::ScreenImage {
                data,
                format,
                is_last,
            } => {
                self.image_buffer.extend_from_slice(&data);
                if is_last {
                    self.show_self_image_buffer(format)?;
                    self.image_buffer.clear();
                }
                Ok(())
            }
            UiMessage::Notification { message, color, .. } => {
                // self.show_notification(color, &message)
                log::info!("[TODO] Showing notification: {}", message);
                Ok(())
            }
            UiMessage::AsrResult(text) => self.input_asr_result(&text),
        }
    }

    pub fn show_self_image_buffer(&mut self, format: ImageFormat) -> anyhow::Result<()> {
        let data = &self.image_buffer;

        match format {
            ImageFormat::Png => {
                let img_reader = image::ImageReader::with_format(
                    std::io::Cursor::new(data),
                    image::ImageFormat::Png,
                );
                let img = img_reader.decode()?.to_rgb8();
                self.draw_rgb888(&img)?;
                self.display.flush()?;
            }
            ImageFormat::Jpeg => {
                display_jpeg(data)?;
            }
            ImageFormat::Gif => {
                // GIF 动画处理可以在这里扩展
                log::warn!("GIF format not fully supported yet");
            }
        }

        Ok(())
    }

    /// 显示图片
    pub fn show_image(&mut self, data: &[u8], format: ImageFormat) -> anyhow::Result<()> {
        match format {
            ImageFormat::Png => {
                let img_reader = image::ImageReader::with_format(
                    std::io::Cursor::new(data),
                    image::ImageFormat::Png,
                );
                let img = img_reader.decode()?.to_rgb8();
                self.draw_rgb888(&img)?;
                self.display.flush()?;
            }
            ImageFormat::Jpeg => {
                display_jpeg(data)?;
            }
            ImageFormat::Gif => {
                // GIF 动画处理可以在这里扩展
                log::warn!("GIF format not fully supported yet");
            }
        }

        Ok(())
    }

    /// 绘制 RGB888 图像数据
    fn draw_rgb888(&mut self, img: &image::RgbImage) -> anyhow::Result<()> {
        self.display.fill_color(ColorFormat::CSS_WHITE)?;

        let pixels = img.enumerate_pixels().map(|(x, y, p)| {
            Pixel(
                Point::new(x as i32, y as i32),
                ColorFormat::new(
                    p[0] / (u8::MAX / ColorFormat::MAX_R),
                    p[1] / (u8::MAX / ColorFormat::MAX_G),
                    p[2] / (u8::MAX / ColorFormat::MAX_B),
                ),
            )
        });

        self.display.draw_iter(pixels)?;
        self.display.fix_background()?;
        Ok(())
    }

    /// 开始输入模式
    pub fn start_input(&mut self, prompt: &str) -> anyhow::Result<()> {
        self.input_mode = true;

        self.input_asr_result(prompt)?;
        Ok(())
    }

    /// 刷新输入显示
    pub fn refresh_input_display(&mut self) -> anyhow::Result<()> {
        // 提取需要的数据，避免借用冲突
        let cursor_pos = self.asr_cursor_pos;

        // 检查麦克风状态
        let is_mic_on = crate::audio::MIC_ON.load(std::sync::atomic::Ordering::Relaxed);

        // 先绘制麦克风状态条
        let y_offset = if is_mic_on {
            let mic_color = ColorFormat::CSS_DARK_GREEN;
            let bounding_box = self.display.bounding_box();
            let top_bar = Rectangle::new(Point::new(0, 0), Size::new(bounding_box.size.width, 14));
            top_bar.draw_styled(&PrimitiveStyle::with_fill(mic_color), &mut self.display)?;
            self.draw_text(
                "● Listening",
                Point::new(0, 2),
                ColorFormat::CSS_WHITE,
                None,
                true,
            )?;
            14
        } else {
            let mic_color = ColorFormat::CSS_DARK_SEA_GREEN;
            let bounding_box = self.display.bounding_box();
            let top_bar = Rectangle::new(Point::new(0, 0), Size::new(bounding_box.size.width, 14));
            top_bar.draw_styled(&PrimitiveStyle::with_fill(mic_color), &mut self.display)?;
            self.draw_text(
                &"Waiting",
                Point::new(4, 2),
                ColorFormat::CSS_WHITE,
                None,
                true,
            )?;
            14
        };

        let display_text = if self.asr_input.is_empty() {
            "\x1b[44m_\x1b[49m".to_string()
        } else {
            let chars: Vec<char> = self.asr_input.chars().collect();
            let mut input_with_cursor = String::new();
            for (i, c) in chars.iter().enumerate() {
                if i == cursor_pos {
                    input_with_cursor.push_str(&format!("\x1b[44m{}\x1b[49m", c));
                } else {
                    input_with_cursor.push(*c);
                }
            }

            if cursor_pos == chars.len() {
                input_with_cursor.push_str("\x1b[44m_\x1b[49m");
            }
            format!("{}", input_with_cursor)
        };

        // 绘制整个输入区域（y_offset 根据麦克风状态调整）
        self.draw_text_wrapped(
            &display_text,
            Point::new(2, y_offset),
            self.config.text_color,
        )?;

        self.display.flush()?;
        Ok(())
    }

    pub fn is_input_mode(&self) -> bool {
        self.input_mode
    }

    pub fn show_notification(&mut self, color: ColorFormat, message: &str) -> anyhow::Result<()> {
        self.draw_text_wrapped(message, Point::new(2, 2), color)?;
        self.display.flush()?;
        Ok(())
    }

    pub fn input_asr_result(&mut self, text: &str) -> anyhow::Result<()> {
        log::info!("Inserting ASR result: {}", text);

        self.input_mode = true;

        // 将字符索引转换为字节索引（支持中文等多字节字符）
        let byte_pos = self
            .asr_input
            .char_indices()
            .nth(self.asr_cursor_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.asr_input.len());

        self.asr_input.insert_str(byte_pos, text);
        self.asr_cursor_pos += text.chars().count();
        self.refresh_input_display()?;
        Ok(())
    }

    /// 向左移动光标
    pub fn move_cursor_left(&mut self) -> anyhow::Result<()> {
        if self.asr_cursor_pos > 0 {
            self.asr_cursor_pos -= 1;
            self.refresh_input_display()?;
        }
        Ok(())
    }

    /// 向右移动光标
    pub fn move_cursor_right(&mut self) -> anyhow::Result<()> {
        let max_pos = self.asr_input.chars().count();
        if self.asr_cursor_pos < max_pos {
            self.asr_cursor_pos += 1;
            self.refresh_input_display()?;
        }
        Ok(())
    }

    pub fn delete_char_before_cursor(&mut self) -> anyhow::Result<()> {
        if self.asr_cursor_pos > 0 {
            // 将字符索引转换为字节索引（支持中文等多字节字符）
            let byte_pos = self
                .asr_input
                .char_indices()
                .nth(self.asr_cursor_pos - 1)
                .map(|(i, _)| i)
                .unwrap_or(0);

            self.asr_input.remove(byte_pos);
            self.asr_cursor_pos -= 1;
            self.refresh_input_display()?;
        }
        Ok(())
    }

    pub fn clear_input(&mut self) -> anyhow::Result<()> {
        self.input_mode = false;
        self.asr_input.clear();
        self.asr_cursor_pos = 0;
        self.refresh_input_display()?;
        Ok(())
    }

    pub fn take_waiting_input_prompt(&mut self) -> String {
        self.asr_cursor_pos = 0;
        self.input_mode = false;

        std::mem::take(&mut self.asr_input)
    }

    // ========== 辅助方法 ==========

    /// 绘制单行文本
    fn draw_text(
        &mut self,
        text: &str,
        position: Point,
        color: ColorFormat,
        bg_color: Option<ColorFormat>,
        centered: bool,
    ) -> anyhow::Result<()> {
        const LINE_HEIGHT: u32 = 14;

        let font = u8g2_fonts::fonts::u8g2_font_boutique_bitmap_7x7_t_gb2312;

        let style = MyTextStyle {
            font_style: U8g2TextStyle::new(font, color),
            vertical_offset: 0,
            bg_color,
        };

        // 使用 TextBox 绘制单行文本 (与 display_text 保持一致)
        let text_box = Rectangle::new(
            position,
            Size::new((DISPLAY_WIDTH as i32 - position.x) as u32, LINE_HEIGHT),
        );

        let alignment = if centered {
            embedded_text::alignment::HorizontalAlignment::Center
        } else {
            embedded_text::alignment::HorizontalAlignment::Left
        };

        let textbox_style = embedded_text::style::TextBoxStyleBuilder::new()
            .height_mode(embedded_text::style::HeightMode::ShrinkToText(
                embedded_text::style::VerticalOverdraw::FullRowsOnly,
            ))
            .alignment(alignment)
            .line_height(embedded_graphics::text::LineHeight::Pixels(LINE_HEIGHT))
            .build();

        embedded_text::TextBox::with_textbox_style(text, text_box, style, textbox_style)
            .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
            .draw(&mut self.display)?;

        Ok(())
    }

    /// 绘制换行文本
    fn draw_text_wrapped(
        &mut self,
        text: &str,
        position: Point,
        color: ColorFormat,
    ) -> anyhow::Result<()> {
        let bounding_box = self.display.bounding_box();
        let text_box = Rectangle::new(
            position,
            Size::new(
                bounding_box.size.width.saturating_sub(position.x as u32),
                bounding_box.size.height.saturating_sub(position.y as u32),
            ),
        );

        let textbox_style = embedded_text::style::TextBoxStyleBuilder::new()
            .height_mode(embedded_text::style::HeightMode::ShrinkToText(
                embedded_text::style::VerticalOverdraw::FullRowsOnly,
            ))
            .alignment(embedded_text::alignment::HorizontalAlignment::Left)
            .line_height(embedded_graphics::text::LineHeight::Pixels(14))
            .build();

        embedded_text::TextBox::with_textbox_style(
            text,
            text_box,
            MyTextStyle {
                font_style: U8g2TextStyle::new(u8g2_fonts::fonts::u8g2_font_wqy12_t_gb2312, color),
                vertical_offset: 3,
                bg_color: None,
            },
            textbox_style,
        )
        .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
        .draw(&mut self.display)?;

        Ok(())
    }
}

impl Default for UI {
    fn default() -> Self {
        Self::new()
    }
}

impl From<crate::protocol::ServerMessage> for UiMessage {
    fn from(msg: crate::protocol::ServerMessage) -> Self {
        match msg {
            crate::protocol::ServerMessage::ScreenImage(data) => {
                let format = match data.format {
                    crate::protocol::ImageFormat::Png => ImageFormat::Png,
                    crate::protocol::ImageFormat::Jpeg => ImageFormat::Jpeg,
                    crate::protocol::ImageFormat::Gif => ImageFormat::Gif,
                };
                UiMessage::ScreenImage {
                    data: data.data,
                    format,
                    is_last: data.is_last,
                }
            }
            crate::protocol::ServerMessage::Notification(data) => {
                let color = match data.level {
                    crate::protocol::NotificationLevel::Info => NotificationLevel::Info.to_color(),
                    crate::protocol::NotificationLevel::Success => {
                        NotificationLevel::Success.to_color()
                    }
                    crate::protocol::NotificationLevel::Warning => {
                        NotificationLevel::Warning.to_color()
                    }
                    crate::protocol::NotificationLevel::Error => {
                        NotificationLevel::Error.to_color()
                    }
                    crate::protocol::NotificationLevel::Custom => {
                        // (None,R,G,B)
                        let color_arr = data.color.to_be_bytes();
                        ColorFormat::new(color_arr[1], color_arr[2], color_arr[3])
                    }
                };
                UiMessage::Notification {
                    color,
                    message: data.message,
                    title: data.title,
                }
            }
            crate::protocol::ServerMessage::Title(text) => UiMessage::Notification {
                color: NotificationLevel::Info.to_color(),
                message: text,
                title: None,
            },
            crate::protocol::ServerMessage::AsrResult(text) => UiMessage::AsrResult(text),
            crate::protocol::ServerMessage::PtyOutput(_) => {
                if cfg!(debug_assertions) {
                    unreachable!("Received PtyOutput message, ignoring in UI conversion")
                } else {
                    UiMessage::Notification {
                        color: NotificationLevel::Warning.to_color(),
                        message: "Received unexpected PtyOutput message".to_string(),
                        title: Some("Warning".to_string()),
                    }
                }
            }
        }
    }
}

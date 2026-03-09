use std::fmt::Debug;

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
            .set_text_color(Some(text_color.unwrap_or(ColorFormat::CSS_WHEAT)));
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

pub const DEFAULT_BACKGROUND: &[u8] = include_bytes!("../assets/lm_320x240.png");

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
                ColorFormat::CSS_BLACK,
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
    },

    /// 通知消息
    Notification {
        color: ColorFormat,
        message: String,
        title: Option<String>,
    },

    /// 请求输入
    GetInput {
        prompt: String,
    },

    /// 提供选择项
    Choices {
        id: String,
        title: String,
        options: Vec<String>,
    },

    /// ASR 结果
    AsrResult(String),

    Status(String),
}

impl Debug for UiMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiMessage::ScreenImage { data, format } => f
                .debug_struct("ScreenImage")
                .field("format", format)
                .field("data_len", &data.len())
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
            UiMessage::GetInput { prompt } => f
                .debug_tuple("GetInput")
                .field(&prompt.chars().take(20).collect::<String>())
                .finish(),
            UiMessage::Choices { id, title, options } => f
                .debug_struct("Choices")
                .field("id", id)
                .field("title", &title.chars().take(20).collect::<String>())
                .field("options_count", &options.len())
                .finish(),
            UiMessage::AsrResult(text) => f
                .debug_tuple("AsrResult")
                .field(&text.chars().take(20).collect::<String>())
                .finish(),
            UiMessage::Status(status) => f
                .debug_tuple("Status")
                .field(&status.chars().take(20).collect::<String>())
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

/// UI 当前状态
#[derive(Clone, Debug)]
pub enum UiState {
    /// 空闲状态
    Idle,

    /// 显示图片
    ShowingImage,

    /// 显示通知
    ShowingNotification { color: ColorFormat, message: String },

    /// 等待输入
    WaitingInput {
        prompt: String,
        current_input: String,
        cursor_pos: usize,
    },

    /// 等待选择
    WaitingChoice {
        id: String,
        title: String,
        options: Vec<String>,
        selected_index: i32,
    },
}

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
            text_color: ColorFormat::CSS_BLACK,
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
    /// 当前 UI 状态
    state: UiState,
    status_bar: String,
    /// UI 配置
    config: UiConfig,
    scroll_offset: i32,

    waiting_input_prompt: String,
}

impl UI {
    /// 创建新的 UI 实例
    pub fn new() -> Self {
        Self {
            display: FrameBuffer::new(ColorFormat::CSS_WHITE),
            state: UiState::Idle,
            config: UiConfig::default(),
            scroll_offset: 0,
            waiting_input_prompt: String::new(),
            status_bar: "[N]".to_string(),
        }
    }

    /// 使用指定显示目标创建 UI
    pub fn new_with_target(display: FrameBuffer) -> Self {
        Self {
            display,
            state: UiState::Idle,
            config: UiConfig::default(),
            scroll_offset: 0,
            waiting_input_prompt: String::new(),
            status_bar: "[N]".to_string(),
        }
    }

    /// 处理 UI 消息 (对应 protocol.rs 的 ServerMessage)
    pub fn handle_message(&mut self, msg: UiMessage) -> anyhow::Result<()> {
        log::info!("Handling UI message: {:?}", msg);
        match msg {
            UiMessage::ScreenImage { data, format } => self.show_image(&data, format),
            UiMessage::Notification { message, color, .. } => {
                self.show_notification(color, &message)
            }
            UiMessage::GetInput { prompt } => self.start_input(&prompt),
            UiMessage::Choices { title, options, id } => self.show_choices(&id, &title, &options),
            UiMessage::AsrResult(text) => self.show_asr_result(&text),
            UiMessage::Status(status) => self.set_status(&status),
        }
    }

    pub fn reset_scroll(&mut self) -> anyhow::Result<()> {
        self.scroll_offset = 0;
        match &self.state {
            UiState::ShowingNotification { .. } => self.refresh_notification(),
            UiState::WaitingChoice { .. } => self.refresh_choices_display(),
            _ => Ok(()),
        }
    }

    pub fn scroll_up(&mut self) -> anyhow::Result<()> {
        self.scroll_offset -= 14;
        match &self.state {
            UiState::ShowingNotification { .. } => self.refresh_notification(),
            UiState::WaitingChoice { .. } => self.refresh_choices_display(),
            _ => Ok(()),
        }
    }

    pub fn scroll_down(&mut self) -> anyhow::Result<()> {
        self.scroll_offset += 14;
        match &self.state {
            UiState::ShowingNotification { .. } => self.refresh_notification(),
            UiState::WaitingChoice { .. } => self.refresh_choices_display(),
            _ => Ok(()),
        }
    }

    /// 显示图片
    pub fn show_image(&mut self, data: &[u8], format: ImageFormat) -> anyhow::Result<()> {
        self.state = UiState::ShowingImage;
        self.scroll_offset = 0;

        match format {
            ImageFormat::Png => {
                let img_reader = image::ImageReader::with_format(
                    std::io::Cursor::new(data),
                    image::ImageFormat::Png,
                );
                let img = img_reader.decode()?.to_rgb8();
                self.draw_rgb888(&img)?;
            }
            ImageFormat::Jpeg => {
                let img_reader = image::ImageReader::with_format(
                    std::io::Cursor::new(data),
                    image::ImageFormat::Jpeg,
                );
                let img = img_reader.decode()?.to_rgb8();
                self.draw_rgb888(&img)?;
            }
            ImageFormat::Gif => {
                // GIF 动画处理可以在这里扩展
                log::warn!("GIF format not fully supported yet");
            }
        }

        self.display.flush()?;
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

    /// 显示通知
    pub fn show_notification(&mut self, color: ColorFormat, message: &str) -> anyhow::Result<()> {
        self.state = UiState::ShowingNotification {
            color,
            message: message.to_string(),
        };
        self.scroll_offset = 0;

        // self.display.fill_color(self.config.notification_bg)?;

        const LINE_HEIGHT: i32 = 14;

        // 绘制顶部颜色条表示级别
        let bounding_box = self.display.bounding_box();
        let top_bar = Rectangle::new(
            Point::new(0, 0),
            Size::new(bounding_box.size.width, LINE_HEIGHT as u32),
        );
        top_bar.draw_styled(&PrimitiveStyle::with_fill(color), &mut self.display)?;

        let status_bar_str = format!("{}", self.status_bar.clone());

        self.draw_text(
            &status_bar_str,
            Point::new(4, 2),
            ColorFormat::CSS_WHEAT,
            false,
        )?;

        // 显示消息
        self.draw_text_wrapped(message, Point::new(2, LINE_HEIGHT), self.config.text_color)?;

        self.display.flush()?;
        Ok(())
    }

    /// 开始输入模式
    pub fn start_input(&mut self, prompt: &str) -> anyhow::Result<()> {
        if matches!(self.state, UiState::WaitingInput { .. }) {
            return Ok(()); // 已经在输入模式，不重复设置
        }

        // TODO: change state bar

        if let UiState::ShowingNotification { color, .. } = &mut self.state {
            *color = ColorFormat::new(255, 150, 0); // 切换到输入模式，先把通知颜色改为橙色
            self.waiting_input_prompt = prompt.to_string();
            return Ok(()); // 正在显示通知，先保存输入提示，等刷新时再切换到输入模式
        }

        if cfg!(debug_assertions) {
            unreachable!("Unexpected state when starting input: {:?}", self.state);
        } else {
            // unreachable in current design, but just in case
            self.state = UiState::WaitingInput {
                prompt: prompt.to_string(),
                current_input: String::new(),
                cursor_pos: 0,
            };
            self.refresh_input_display()?;
            Ok(())
        }
    }

    /// 刷新输入显示
    fn refresh_input_display(&mut self) -> anyhow::Result<()> {
        // 提取需要的数据，避免借用冲突
        let (prompt, current_input, cursor_pos) = if let UiState::WaitingInput {
            prompt,
            current_input,
            cursor_pos,
        } = &self.state
        {
            (prompt.clone(), current_input.clone(), *cursor_pos)
        } else {
            return Ok(());
        };

        // 检查麦克风状态
        let is_mic_on = crate::audio::MIC_ON.load(std::sync::atomic::Ordering::Relaxed);

        // 先绘制麦克风状态条
        let y_offset = if is_mic_on {
            let mic_color = ColorFormat::new(255, 50, 50); // 红色表示录音中
            let bounding_box = self.display.bounding_box();
            let top_bar = Rectangle::new(Point::new(0, 0), Size::new(bounding_box.size.width, 10));
            top_bar.draw_styled(&PrimitiveStyle::with_fill(mic_color), &mut self.display)?;
            self.draw_text("● Recording", Point::new(0, 0), mic_color, true)?;
            10
        } else {
            let mic_color = ColorFormat::new(50, 255, 50); // 绿色表示空闲
            let bounding_box = self.display.bounding_box();
            let top_bar = Rectangle::new(Point::new(0, 0), Size::new(bounding_box.size.width, 10));
            top_bar.draw_styled(&PrimitiveStyle::with_fill(mic_color), &mut self.display)?;
            let status_bar_str = self.status_bar.clone();
            self.draw_text(&status_bar_str, Point::new(0, 0), mic_color, false)?;
            10
        };

        // 使用 ANSI 代码标记光标位置, prompt 用灰色背景
        let display_text = if current_input.is_empty() {
            // 空输入时显示光标标记
            format!("\x1b[48;5;240m{}\x1b[49m\x1b[44m_\x1b[49m", prompt) // prompt 灰色背景，光标蓝色背景
        } else {
            let chars: Vec<char> = current_input.chars().collect();
            let mut input_with_cursor = String::new();
            for (i, c) in chars.iter().enumerate() {
                if i == cursor_pos {
                    // 光标位置：用蓝色背景标记
                    input_with_cursor.push_str(&format!("\x1b[44m{}\x1b[49m", c));
                } else {
                    input_with_cursor.push(*c);
                }
            }
            // 如果光标在末尾，添加光标标记
            if cursor_pos == chars.len() {
                input_with_cursor.push_str("\x1b[44m_\x1b[49m");
            }
            format!("\x1b[48;5;240m{}\x1b[49m\n{}", prompt, input_with_cursor) // prompt 灰色背景
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

    #[allow(unused)]
    /// 添加输入字符（在光标位置插入）
    pub fn add_input_char(&mut self, c: char) -> anyhow::Result<()> {
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            let mut chars: Vec<char> = current_input.chars().collect();
            chars.insert(*cursor_pos, c);
            *current_input = chars.into_iter().collect();
            *cursor_pos += 1;
            self.refresh_input_display()?;
        }
        Ok(())
    }

    /// 删除光标前的字符（backspace）
    pub fn remove_input_char(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            if *cursor_pos > 0 {
                let mut chars: Vec<char> = current_input.chars().collect();
                chars.remove(*cursor_pos - 1);
                *current_input = chars.into_iter().collect();
                *cursor_pos -= 1;
                self.refresh_input_display()?;
            }
        }
        Ok(())
    }

    #[allow(unused)]
    /// 删除光标后的字符（delete）
    pub fn delete_char_at_cursor(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            let mut chars: Vec<char> = current_input.chars().collect();
            if *cursor_pos < chars.len() {
                chars.remove(*cursor_pos);
                *current_input = chars.into_iter().collect();
                self.refresh_input_display()?;
            }
        }
        Ok(())
    }

    /// 光标左移
    pub fn move_cursor_left(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingInput { cursor_pos, .. } = &mut self.state {
            *cursor_pos = cursor_pos.saturating_sub(1);
            self.refresh_input_display()?;
        }
        Ok(())
    }

    /// 光标右移
    pub fn move_cursor_right(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            let max_pos = current_input.chars().count();
            *cursor_pos = (*cursor_pos + 1).min(max_pos);
            self.refresh_input_display()?;
        }
        Ok(())
    }

    /// 在光标位置插入文本（用于 ASR 结果）
    pub fn insert_text_at_cursor(&mut self, text: &str) -> anyhow::Result<()> {
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            let mut chars: Vec<char> = current_input.chars().collect();
            let insert_chars: Vec<char> = text.chars().collect();
            for c in insert_chars {
                chars.insert(*cursor_pos, c);
                *cursor_pos += 1;
            }
            *current_input = chars.into_iter().collect();
            self.refresh_input_display()?;
        }
        Ok(())
    }

    pub fn insert_text_at_start(&mut self, text: &str) -> anyhow::Result<()> {
        if let UiState::WaitingInput { current_input, .. } = &mut self.state {
            *current_input = format!("{}{}", text, current_input);
            self.refresh_input_display()?;
        }
        Ok(())
    }

    /// 获取当前输入并返回
    pub fn get_input(&self) -> Option<String> {
        if let UiState::WaitingInput { current_input, .. } = &self.state {
            Some(current_input.clone())
        } else {
            None
        }
    }

    /// 清空当前输入
    pub fn clear_input(&mut self) -> anyhow::Result<()> {
        self.scroll_offset = 0;
        if let UiState::WaitingInput {
            current_input,
            cursor_pos,
            ..
        } = &mut self.state
        {
            *current_input = String::new();
            *cursor_pos = 0;
            self.refresh_input_display()?;
        }
        Ok(())
    }

    #[allow(unused)]
    /// 获取光标位置
    pub fn get_cursor_pos(&self) -> Option<usize> {
        if let UiState::WaitingInput { cursor_pos, .. } = &self.state {
            Some(*cursor_pos)
        } else {
            None
        }
    }

    /// 刷新输入界面（用于麦克风状态变化时）
    pub fn refresh_input_if_waiting(&mut self) -> anyhow::Result<()> {
        match self.state {
            UiState::WaitingInput { .. } => self.refresh_input_display(),
            _ => {
                if self.waiting_input_prompt.is_empty() {
                    Ok(())
                } else {
                    // 之前正在显示通知时收到输入请求，先切换到输入模式
                    self.state = UiState::WaitingInput {
                        prompt: self.take_waiting_input_prompt(),
                        current_input: String::new(),
                        cursor_pos: 0,
                    };
                    self.refresh_input_display()
                }
            }
        }
    }

    /// 显示选择项
    pub fn show_choices(
        &mut self,
        id: &str,
        title: &str,
        options: &[String],
    ) -> anyhow::Result<()> {
        if let UiState::WaitingChoice {
            id: existing_id, ..
        } = &self.state
        {
            if existing_id == id {
                return Ok(()); // 已经在选择模式，不重复设置
            }
        }

        self.state = UiState::WaitingChoice {
            id: id.to_string(),
            title: title.to_string(),
            options: options.to_vec(),
            selected_index: 0,
        };
        self.refresh_choices_display()?;
        Ok(())
    }

    /// 刷新选择项显示
    fn refresh_choices_display(&mut self) -> anyhow::Result<()> {
        // 提取需要的数据，避免借用冲突
        let (title, options, selected_index) = if let UiState::WaitingChoice {
            title,
            options,
            selected_index,
            ..
        } = &self.state
        {
            (title.clone(), options.clone(), *selected_index)
        } else {
            return Ok(());
        };

        // 使用 ANSI 代码构建选择项显示文本
        // 标题用灰色背景，选中项用蓝色背景
        let mut display_text = format!("{}\n", title);

        // 空选项时显示 Confirm/Cancel
        let render_options = if options.is_empty() {
            vec![
                "Confirm ([Accept])".to_string(),
                "Cancel ([ESC])".to_string(),
            ]
        } else {
            options
        };

        for (i, option) in render_options.iter().enumerate() {
            if i as i32 == selected_index {
                // 选中项：蓝色背景，白色文字
                display_text.push_str(&format!("\x1b[44;37m[ {} ]\x1b[49m\n", option));
            } else {
                // 未选中项：普通文字
                display_text.push_str(&format!(" {}\n", option));
            }
        }

        self.draw_text_wrapped(&display_text, Point::new(2, 2), self.config.text_color)?;

        self.display.flush()?;
        Ok(())
    }

    /// 选择下一项（可循环）
    pub fn next_choice(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingChoice {
            options,
            selected_index,
            ..
        } = &mut self.state
        {
            // 空选项时不做处理（Confirm/Cancel 只能通过按键选择）
            if options.is_empty() {
                return Ok(());
            }
            *selected_index = (*selected_index + 1) % options.len() as i32;
            self.refresh_choices_display()?;
        }
        Ok(())
    }

    #[allow(unused)]
    /// 选择上一项（可循环）
    pub fn prev_choice(&mut self) -> anyhow::Result<()> {
        if let UiState::WaitingChoice {
            options,
            selected_index,
            ..
        } = &mut self.state
        {
            // 空选项时不做处理（Confirm/Cancel 只能通过按键选择）
            if options.is_empty() {
                return Ok(());
            }
            let len = options.len();
            *selected_index = if *selected_index == 0 {
                (len - 1) as i32
            } else {
                *selected_index - 1
            };
            self.refresh_choices_display()?;
        }
        Ok(())
    }

    /// 确认选择并返回选中的索引
    pub fn confirm_choice(&self) -> Option<i32> {
        if let UiState::WaitingChoice { selected_index, .. } = &self.state {
            Some(*selected_index)
        } else {
            None
        }
    }

    /// 检查是否为 confirm/cancel 对话框（空选项）
    pub fn is_confirm_dialog(&self) -> bool {
        if let UiState::WaitingChoice { options, .. } = &self.state {
            options.is_empty()
        } else {
            false
        }
    }

    /// 刷新通知显示（用于滚动）
    pub fn refresh_notification(&mut self) -> anyhow::Result<()> {
        if let UiState::ShowingNotification { color, message } = &self.state {
            // self.display.fill_color(self.config.notification_bg)?;
            let message = message.clone();
            let color = *color;

            const LINE_HEIGHT: i32 = 14;

            // 绘制顶部颜色条表示级别
            let bounding_box = self.display.bounding_box();
            let top_bar = Rectangle::new(
                Point::new(0, 0),
                Size::new(bounding_box.size.width, LINE_HEIGHT as u32),
            );
            top_bar.draw_styled(&PrimitiveStyle::with_fill(color), &mut self.display)?;

            let status_bar_str = self.status_bar.clone();

            self.draw_text(
                &status_bar_str,
                Point::new(4, 2),
                ColorFormat::CSS_WHEAT,
                false,
            )?;

            let y_offset = LINE_HEIGHT;

            // 显示消息
            self.draw_text_wrapped(&message, Point::new(2, y_offset), self.config.text_color)?;

            self.display.flush()?;
        }
        Ok(())
    }

    /// 显示 ASR 结果（如果在输入模式，直接插入到光标位置）
    pub fn show_asr_result(&mut self, text: &str) -> anyhow::Result<()> {
        // 如果当前在输入模式，直接插入到光标位置
        self.scroll_offset = 0;

        if matches!(self.state, UiState::WaitingInput { .. }) {
            self.insert_text_at_cursor(text)
        } else if !self.waiting_input_prompt.is_empty() || matches!(self.state, UiState::Idle) {
            // 如果之前显示过文本或通知，并且有未进入输入模式的提示，先切换到输入模式再插入
            self.state = UiState::WaitingInput {
                prompt: self.take_waiting_input_prompt(),
                current_input: String::new(),
                cursor_pos: 0,
            };
            self.insert_text_at_cursor(text)
        } else {
            // 不在输入模式时，可以显示一个简短的通知
            let preview = if text.chars().count() > 20 {
                format!("{}...", text.chars().take(17).collect::<String>())
            } else {
                text.to_string()
            };
            self.show_notification(NotificationLevel::Info.to_color(), &preview)
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> &UiState {
        &self.state
    }

    pub fn set_status(&mut self, status: &str) -> anyhow::Result<()> {
        self.status_bar = status.to_string();
        match &self.state {
            UiState::WaitingInput { .. } => self.refresh_input_display(),
            _ => Ok(()),
        }
    }

    fn take_waiting_input_prompt(&mut self) -> String {
        std::mem::take(&mut self.waiting_input_prompt)
    }

    /// 清屏并重置到空闲状态
    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.state = UiState::Idle;
        self.display.fill_color(ColorFormat::CSS_WHITE)?;
        self.display.flush()?;
        Ok(())
    }

    // ========== 辅助方法 ==========

    /// 绘制单行文本
    fn draw_text(
        &mut self,
        text: &str,
        position: Point,
        color: ColorFormat,
        centered: bool,
    ) -> anyhow::Result<()> {
        const LINE_HEIGHT: u32 = 14;

        let font = u8g2_fonts::fonts::u8g2_font_boutique_bitmap_7x7_t_gb2312;

        let style = MyTextStyle {
            font_style: U8g2TextStyle::new(font, color),
            vertical_offset: 0,
            bg_color: Some(ColorFormat::CSS_BLACK),
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
        .set_vertical_offset(self.scroll_offset)
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
            crate::protocol::ServerMessage::GetInput(data) => UiMessage::GetInput {
                prompt: data.prompt,
            },
            crate::protocol::ServerMessage::Choices(data) => UiMessage::Choices {
                id: data.id.unwrap_or_default(),
                title: data.title,
                options: data.options,
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
            crate::protocol::ServerMessage::Status(s) => UiMessage::Status(s),
        }
    }
}

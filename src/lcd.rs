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

#[cfg(feature = "max2")]
pub const DISPLAY_WIDTH: usize = 320;
#[cfg(feature = "max2")]
pub const DISPLAY_HEIGHT: usize = 172;

#[cfg(not(feature = "max2"))]
pub const DISPLAY_WIDTH: usize = 284;
#[cfg(not(feature = "max2"))]
pub const DISPLAY_HEIGHT: usize = 78;

static mut ESP_LCD_PANEL_HANDLE: esp_idf_svc::sys::esp_lcd_panel_handle_t = std::ptr::null_mut();
pub type ColorFormat = Rgb565;

pub fn init_spi(_spi: SPI3, mosi: Gpio21, clk: Gpio47) -> Result<(), EspError> {
    use esp_idf_svc::hal::spi::Spi;
    use esp_idf_svc::sys::*;
    const GPIO_NUM_NC: i32 = -1;

    let mut buscfg = spi_bus_config_t::default();
    buscfg.__bindgen_anon_1.mosi_io_num = mosi.pin() as _;
    buscfg.__bindgen_anon_2.miso_io_num = GPIO_NUM_NC;
    buscfg.sclk_io_num = clk.pin() as _;
    buscfg.__bindgen_anon_3.quadwp_io_num = GPIO_NUM_NC;
    buscfg.__bindgen_anon_4.quadhd_io_num = GPIO_NUM_NC;
    buscfg.max_transfer_sz = 1024 * 4;
    esp!(unsafe { spi_bus_initialize(SPI3::device(), &buscfg, spi_common_dma_t_SPI_DMA_CH_AUTO,) })
}

pub fn init_lcd(cs: Gpio12, dc: Gpio13, rst: Gpio14) -> Result<(), EspError> {
    use esp_idf_svc::sys::*;

    ::log::info!("Install panel IO");
    let mut panel_io: esp_lcd_panel_io_handle_t = std::ptr::null_mut();
    let mut io_config = esp_lcd_panel_io_spi_config_t::default();
    io_config.cs_gpio_num = cs.pin() as _;
    io_config.dc_gpio_num = dc.pin() as _;
    io_config.spi_mode = 3;
    io_config.pclk_hz = 60 * 1000 * 1000;
    io_config.trans_queue_depth = 10;
    io_config.lcd_cmd_bits = 8;
    io_config.lcd_param_bits = 8;
    esp!(unsafe {
        esp_lcd_new_panel_io_spi(spi_host_device_t_SPI3_HOST as _, &io_config, &mut panel_io)
    })?;

    ::log::info!("Install LCD driver");

    let mut panel_config = esp_lcd_panel_dev_config_t::default();
    let mut panel: esp_lcd_panel_handle_t = std::ptr::null_mut();

    panel_config.reset_gpio_num = rst.pin() as _;
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
    #[cfg(feature = "max2")]
    const DISPLAY_INVERT_COLOR: bool = true;
    #[cfg(not(feature = "max2"))]
    const DISPLAY_INVERT_COLOR: bool = false;

    ::log::info!("Reset LCD panel");
    unsafe {
        if cfg!(feature = "max2") {
            esp!(esp_lcd_panel_set_gap(panel, 0, 34))?;
        } else {
            esp!(esp_lcd_panel_set_gap(panel, 18, 82))?;
        }
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

impl FrameBuffer {
    /// 清空可绘制缓冲区(背景快照缓冲不动)。终端全屏帧重绘前用它清屏。
    pub fn clear(&mut self, color: ColorFormat) -> anyhow::Result<()> {
        self.buffers.clear(color).map_err(Into::into)
    }

    /// 只把指定矩形区域推送到 LCD(增量重绘用),不刷新整屏。
    /// `rect` 会被裁剪到屏幕范围内。
    pub fn flush_rect(&mut self, rect: Rectangle) -> anyhow::Result<()> {
        let bb = self.bounding_box();
        let r = rect.intersection(&bb);
        if r.size.width == 0 || r.size.height == 0 {
            return Ok(());
        }
        let data = self.buffers.data();
        let x0 = r.top_left.x as usize;
        let y0 = r.top_left.y as usize;
        let x1 = x0 + r.size.width as usize;
        let y1 = y0 + r.size.height as usize;
        let w = DISPLAY_WIDTH;
        let mut sub: Vec<u8> = Vec::with_capacity((x1 - x0) * (y1 - y0) * 2);
        for y in y0..y1 {
            let s = (y * w + x0) * 2;
            let e = (y * w + x1) * 2;
            sub.extend_from_slice(&data[s..e]);
        }
        let xe = r.top_left.x + r.size.width as i32;
        let ye = r.top_left.y + r.size.height as i32;
        for i in 0..5 {
            let code = flush_display(&sub, r.top_left.x, r.top_left.y, xe, ye);
            if code == 0 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            if i < 4 {
                log::warn!("flush_rect retry {}", i + 1);
            } else {
                log::error!("flush_rect failed after retries, code={}", code);
            }
        }
        anyhow::bail!("flush_rect failed after retries")
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

        for i in 0..5 {
            let e = flush_display(self.buffers.data(), x_start, y_start, x_end, y_end);
            if e != 0 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                crate::log_heap();
                if i < 4 {
                    log::warn!(
                        "flush_display failed (attempt {}), retrying... error code: {}",
                        i + 1,
                        e
                    );
                } else {
                    log::error!(
                        "flush_display failed after {} attempts. error code: {}",
                        i + 1,
                        e
                    );
                    anyhow::bail!("Failed to flush display after multiple attempts");
                }
                continue;
            }
            // 成功就跳出,避免每次 flush 都把整屏发 5 遍(5× SPI 带宽)。
            break;
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

pub fn display_jpeg(jpeg: &[u8]) -> anyhow::Result<()> {
    let jpeg_buffer = crate::new_jpg::esp_jpeg_decode_one_picture(jpeg)?;
    log::info!(
        "JPEG decoded: width={}, height={}",
        jpeg_buffer.width,
        jpeg_buffer.height
    );
    jpeg_buffer.flush_to_lcd()
}

// 旧的整屏文本渲染;状态/反馈画面已统一改用 ui::render_keyboard_view,暂留备用。
#[allow(dead_code)]
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

// ========== UI 管理器 ==========

/// UI 管理器
///
/// 负责管理 LCD 显示和用户交互；入站屏幕帧由 mqtt 侧组装成
/// [`crate::protocol::ScreenImageChunk`] 后交给这里渲染。
pub struct UI {
    /// 显示缓冲区
    display: FrameBuffer,
    /// text 模式终端:vt100 解析器(画布 = 3 屏高)。JPEG 模式 / 未选会话时为 None。
    terminal_parser: Option<vt100::Parser>,
    /// text 模式终端渲染器(按 3 屏高创建,字形缓存)。take()/放回 复用。
    terminal_renderer: Option<embedded_graphics_terminal::TerminalRenderer>,
    /// 当前窗口顶部在画布中的行号(本地平移用)。0 = 画布最老;底部 = canvas_rows − visible(最新)。
    terminal_offset: u16,
}

/// 终端画布是物理屏高的几倍。renderer/parser/sync 都按这个高度,本地用 render_rows 只显示
/// 其中一屏窗口([offset, offset+visible) 平移到 y=0);窗口到顶了再向服务端要更早的历史。
/// max2 屏大(172px)用 3 屏;窄屏 keys(78px)更矮,用 5 屏给足可平移的历史。
#[cfg(feature = "max2")]
const TERMINAL_TALL: u32 = 3;
#[cfg(not(feature = "max2"))]
const TERMINAL_TALL: u32 = 5;
/// vt100 解析器保留的画布外历史行数。本地平移在 3 屏画布内进行,更老的走服务端,故设 0。
const TERMINAL_SCROLLBACK_ROWS: usize = 0;

/// 终端滚动方向(本地窗口平移)。
#[derive(Clone, Copy)]
pub enum TerminalScroll {
    Up,
    Down,
}

/// 终端画布总高(像素)= TERMINAL_TALL × 屏高。renderer 按这个尺寸创建。
fn terminal_canvas_height() -> u32 {
    DISPLAY_HEIGHT as u32 * TERMINAL_TALL
}
/// 一个 cell 的像素高(取自渲染器字体)。
fn terminal_cell_h() -> u32 {
    new_terminal_renderer().cell_size().1
}
/// 一屏可见的终端行数(屏高 ÷ cell 高)。
fn terminal_visible_rows() -> u16 {
    (DISPLAY_HEIGHT as u32 / terminal_cell_h()) as u16
}

/// 构造终端渲染器:unifont(含中文 gb2312)+ 符号/拉丁回退字体,黑白配色,
/// 常见非 BMP 符号替换成 ASCII 避免缺字。与 vibetty text 模式配色一致(白字黑底)。
/// **按 3 屏高创建**(`Size` 用画布高),故 rows() = 3×可见;render_rows 取一屏窗口。
fn new_terminal_renderer() -> embedded_graphics_terminal::TerminalRenderer {
    use embedded_graphics_terminal::TerminalRenderer;
    use u8g2_fonts::fonts::{
        u8g2_font_unifont_t_78_79, u8g2_font_unifont_t_gb2312, u8g2_font_unifont_t_symbols,
    };

    TerminalRenderer::new(
        Size::new(DISPLAY_WIDTH as u32, terminal_canvas_height()),
        u8g2_font_unifont_t_gb2312,
        ColorFormat::WHITE,
        ColorFormat::BLACK,
    )
    .with_fallback_font(u8g2_font_unifont_t_symbols)
    .with_fallback_font(u8g2_font_unifont_t_78_79)
    .with_substitution('›', '>')
    .with_substitution('•', '*')
    .with_substitution('✻', '*')
    .with_substitution('⏺', '*')
}

/// text 模式终端的字符列/行数(= 画布尺寸:cols × 3×可见行)。
/// renderer 按 3 屏高创建,故 rows() 返回 3×可见行;sync_cells 与 parser 都用这个。
pub fn terminal_text_cells() -> (u16, u16) {
    let renderer = new_terminal_renderer();
    (renderer.cols() as u16, renderer.rows() as u16)
}

impl UI {
    /// 借出底层 FrameBuffer,供外部直接绘制(如模式外壳)。
    pub fn display_mut(&mut self) -> &mut FrameBuffer {
        &mut self.display
    }

    /// 创建新的 UI 实例
    pub fn new() -> Self {
        Self {
            display: FrameBuffer::new(ColorFormat::new(30, 30, 30)),
            terminal_parser: None,
            terminal_renderer: None,
            terminal_offset: 0,
        }
    }

    /// 使用指定显示目标创建 UI
    pub fn new_with_target(display: FrameBuffer) -> Self {
        Self {
            display,
            terminal_parser: None,
            terminal_renderer: None,
            terminal_offset: 0,
        }
    }

    pub fn show_notification(&mut self, color: ColorFormat, message: &str) -> anyhow::Result<()> {
        self.draw_text_wrapped(message, Point::new(2, 2), color)?;
        self.display.flush()?;
        Ok(())
    }

    // ========== text 模式终端渲染 ==========

    /// 当前是否持有 text 终端(JPEG 模式 / 未激活会话时为 false)。
    /// app 据此决定 sync 发像素还是 cells、滚动走本地还是 MQTT。
    pub fn terminal_active(&self) -> bool {
        self.terminal_parser.is_some()
    }

    /// 渲染一帧 screen_text。payload 首字节是 tag:
    /// `0x00` = 整屏基线(重置 vt100 解析器后重放,含 ANSI 颜色/光标),
    /// `0x01` = PTY 增量(直接喂进解析器)。后续字节是 ANSI 终端流。
    ///
    /// 无论哪种,都 `process` 后用 `render_rows` 重画当前窗口 `[offset, offset+visible)`
    /// (lib 把该范围平移到 y=0),整屏 flush。delta 帧也走整窗重画(不维护脏区,
    /// 实现简单;PTY 输出频繁时靠字形缓存扛)。
    ///
    /// `snap_top`:全屏基线时窗口对齐到哪里——
    /// - false(sync 首帧 / scroll_up 响应):offset=bottom,看最新 / 旧页底;
    /// - true(scroll_down 响应,新页是更新的内容):offset=0,看新页顶
    ///   (新页顶接旧页底,连续向下阅读,与 JPEG 翻页一致)。
    pub fn show_terminal_text_frame(
        &mut self,
        payload: &[u8],
        snap_top: bool,
    ) -> anyhow::Result<()> {
        let Some((&tag, bytes)) = payload.split_first() else {
            log::warn!("empty screen_text frame");
            return Ok(());
        };
        let (cols, rows) = terminal_text_cells(); // rows = 3×可见(画布高)
        let visible = terminal_visible_rows();
        let bottom = rows.saturating_sub(visible);
        match tag {
            0x00 => {
                log::info!(
                    "screen_text full frame: {}B (snap_top={})",
                    bytes.len(),
                    snap_top
                );
                self.terminal_parser =
                    Some(vt100::Parser::new(rows, cols, TERMINAL_SCROLLBACK_ROWS));
                // 全屏基线 = 新页;scroll_down 来的新页看顶(0),其余看底(bottom)。
                self.terminal_offset = if snap_top { 0 } else { bottom };
            }
            0x01 => {
                log::debug!("screen_text delta frame: {}B", bytes.len());
                if self.terminal_parser.is_none() {
                    log::warn!("screen_text delta before full frame; creating blank terminal");
                    self.terminal_parser =
                        Some(vt100::Parser::new(rows, cols, TERMINAL_SCROLLBACK_ROWS));
                    self.terminal_offset = bottom;
                }
            }
            other => {
                log::warn!("unknown screen_text tag: {other}");
                return Ok(());
            }
        }

        if let Some(parser) = self.terminal_parser.as_mut() {
            parser.process(bytes);
        }

        self.render_terminal_window()?;
        self.display.flush()?;
        Ok(())
    }

    /// 用当前 `terminal_offset` 把画布的 `[offset, offset+visible)` 一屏重画到 display
    /// (render_rows 平移到 y=0)。供帧到达 / 本地平移 / 恢复显示复用。
    fn render_terminal_window(&mut self) -> anyhow::Result<()> {
        let Some(parser) = self.terminal_parser.as_ref() else {
            return Ok(());
        };
        let visible = terminal_visible_rows();
        let start = self.terminal_offset;
        let end = (start + visible).min(terminal_text_cells().1);
        let mut renderer = self
            .terminal_renderer
            .take()
            .unwrap_or_else(new_terminal_renderer);
        self.display.clear(ColorFormat::CSS_BLACK)?;
        renderer.render_rows(parser.screen(), &mut self.display, start, end)?;
        renderer.invalidate();
        self.terminal_renderer = Some(renderer);
        Ok(())
    }

    /// 本地平移 text 终端窗口(改 `terminal_offset`,在 3 屏画布内上下移动可见窗)。
    /// 成功移动返回 true;已在画布顶/底(偏移未变)返回 false——调用方据此回退到 MQTT
    /// 翻页(到顶发 scroll_up 向服务端要更早的历史)。
    pub fn scroll_terminal_text(&mut self, direction: TerminalScroll) -> anyhow::Result<bool> {
        if self.terminal_parser.is_none() {
            return Ok(false);
        }
        let visible = terminal_visible_rows();
        let canvas_rows = terminal_text_cells().1;
        let bottom = canvas_rows.saturating_sub(visible);
        let before = self.terminal_offset;
        // 每格滚半屏(行对齐)。
        let step = (visible / 2).max(1);
        let next = match direction {
            TerminalScroll::Up => before.saturating_sub(step), // 看更老 → 窗口上移
            TerminalScroll::Down => before.saturating_add(step), // 看更新 → 窗口下移
        }
        .min(bottom);
        if next == before {
            return Ok(false); // 已到画布顶/底
        }

        log::info!("local text pan: offset {before} -> {next}");
        self.terminal_offset = next;
        self.render_terminal_window()?;
        self.display.flush()?;
        Ok(true)
    }

    /// 用缓存的 vt100 screen 整窗重绘终端(当前 offset 的可见窗 + flush)。
    /// 用于其它画面(ASR 编辑器、弹窗)覆盖过终端后恢复显示,或重连后强制对齐。
    #[allow(dead_code)] // 当前编辑退出 / 重连都用 send_active_sync 取新鲜帧;保留备用。
    pub fn redraw_cached_terminal_text(&mut self) -> anyhow::Result<bool> {
        if self.terminal_parser.is_none() {
            return Ok(false);
        }
        self.render_terminal_window()?;
        self.display.flush()?;
        Ok(true)
    }

    /// 丢弃 text 终端状态(切到 JPEG 会话 / 退订 text 屏时调用,释放内存)。
    pub fn clear_terminal(&mut self) {
        self.terminal_parser = None;
        self.terminal_offset = 0;
        // renderer 保留复用(字形缓存);若担心内存可一并清空,这里保守保留。
    }

    // ========== 辅助方法 ==========

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

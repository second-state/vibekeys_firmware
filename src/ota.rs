use esp_idf_svc::{
    hal::gpio::{AnyIOPin, PinDriver},
    http::server::EspHttpServer,
    io::Write,
};

mod wifi;

type AnyBtn = PinDriver<'static, esp_idf_svc::hal::gpio::AnyIOPin, esp_idf_svc::hal::gpio::Input>;

fn new_btn(
    pin: AnyIOPin,
    pull: esp_idf_svc::hal::gpio::Pull,
    interrupt: esp_idf_svc::hal::gpio::InterruptType,
) -> anyhow::Result<AnyBtn> {
    let mut btn = PinDriver::input(pin)?;
    btn.set_pull(pull)?;
    btn.set_interrupt_type(interrupt)?;
    Ok(btn)
}

pub fn goto_next_firmware() -> anyhow::Result<()> {
    use esp_idf_svc::sys::{esp_ota_get_next_update_partition, esp_ota_set_boot_partition};

    unsafe {
        let partition = esp_ota_get_next_update_partition(std::ptr::null());
        esp_idf_svc::sys::esp!(esp_ota_set_boot_partition(partition))?;
    };

    esp_idf_svc::hal::reset::restart();
}

#[derive(Debug, Clone)]
pub struct Setting {
    pub ssid: String,
    pub pass: String,
}

impl Setting {
    pub fn load_from_nvs(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<Self> {
        let mut str_buf = [0; 128];

        let ssid = nvs
            .get_str("ssid", &mut str_buf)
            .map_err(|e| log::error!("Failed to get ssid: {:?}", e))
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();

        let pass = nvs
            .get_str("pass", &mut str_buf)
            .map_err(|e| log::error!("Failed to get pass: {:?}", e))
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();

        Ok(Setting { ssid, pass })
    }
}

fn main() -> anyhow::Result<()> {
    use embedded_graphics::prelude::RgbColor;
    use lcd::DisplayTargetDrive;

    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // if let Err(e) = ota_main() {
    //     log::error!("OTA main failed: {:?}", e);
    // }

    let partition = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;
    let nvs = esp_idf_svc::nvs::EspDefaultNvs::new(partition, "setting", true)?;

    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;

    let mut bl = esp_idf_svc::hal::gpio::PinDriver::output(peripherals.pins.gpio11)?;
    bl.set_low()?;

    lcd::init_spi(
        peripherals.spi3,
        peripherals.pins.gpio21,
        peripherals.pins.gpio47,
    )?;

    lcd::init_lcd(
        peripherals.pins.gpio12,
        peripherals.pins.gpio13,
        peripherals.pins.gpio14,
    )?;

    let mut target = lcd::FrameBuffer::new(lcd::ColorFormat::WHITE);
    target.flush()?;

    let btn3 = new_btn(
        peripherals.pins.gpio3.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    for i in 0..5 {
        lcd::display_text(
            &mut target,
            format!(
                "OTA Mode\n Press ESC within {} seconds to exit OTA mode.",
                5 - i
            )
            .as_str(),
            0,
        )?;

        for _ in 0..10 {
            if btn3.is_low() {
                log::info!("ESC button pressed, exiting OTA mode");
                goto_next_firmware()?;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    lcd::display_text(&mut target, "OTA Mode\n Connect wifi", 0)?;

    let setting = Setting::load_from_nvs(&nvs)?;
    let mut esp_wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;

    wifi::connect(&mut esp_wifi, &setting.ssid, &setting.pass, sysloop)?;

    let ip = esp_wifi.sta_netif().get_ip_info()?;
    log::info!("Connected to WiFi, IP address: {}", ip.ip);
    lcd::display_text(
        &mut target,
        &format!("OTA Mode\n Connect http://{}", ip.ip),
        0,
    )?;

    let (tx, rx) = std::sync::mpsc::channel::<OtaEvent>();

    let _http_server = ota_http_server(tx)?;

    ota_task(rx)?;

    Ok(())
}

static INDEX_HTML: &str = include_str!("../assets/ota_index.html");

enum OtaEvent {
    DataChunk(Vec<u8>),
    Complete,
}

fn ota_task(rx: std::sync::mpsc::Receiver<OtaEvent>) -> anyhow::Result<()> {
    let mut ota = esp_idf_svc::ota::EspOta::new()?;

    ota.mark_running_slot_valid()?;
    let mut ota_update = ota.initiate_update()?;

    loop {
        match rx.recv() {
            Ok(OtaEvent::DataChunk(data)) => {
                log::info!("Received OTA data chunk of size: {} bytes", data.len());
                ota_update.write(&data).map_err(|e| {
                    log::error!("Failed to write OTA chunk: {:?}", e);
                    anyhow::anyhow!("Failed to write OTA chunk: {:?}", e)
                })?;
            }
            Ok(OtaEvent::Complete) => {
                break;
            }
            Err(e) => {
                log::error!("OTA data channel closed: {:?}", e);
                break;
            }
        }
    }

    ota_update.complete().map_err(|e| {
        log::error!("Failed to complete OTA update: {:?}", e);
        anyhow::anyhow!("Failed to complete OTA update: {:?}", e)
    })?;

    esp_idf_svc::hal::reset::restart();
}

fn ota_http_server(
    tx: std::sync::mpsc::Sender<OtaEvent>,
) -> anyhow::Result<EspHttpServer<'static>> {
    use esp_idf_svc::http::server::Method;

    let mut server = create_server()?;

    server.fn_handler("/ota", Method::Put, move |mut request| {
        let mut buf = vec![0; 4096];

        let mut nn = 0;

        loop {
            let n = request.read(&mut buf).map_err(|e| {
                log::error!("Failed to read request body: {:?}", e);
                anyhow::anyhow!("Failed to read request body: {:?}", e)
            })?;

            nn += n;

            if n == 0 {
                break;
            }

            tx.send(OtaEvent::DataChunk(buf[..n].to_vec()))
                .map_err(|e| {
                    log::error!("Failed to send OTA data chunk: {:?}", e);
                    anyhow::anyhow!("Failed to send OTA data chunk: {:?}", e)
                })?;
        }

        tx.send(OtaEvent::Complete).map_err(|e| {
            log::error!("Failed to send OTA complete event: {:?}", e);
            anyhow::anyhow!("Failed to send OTA complete event: {:?}", e)
        })?;

        let mut resp = request.into_ok_response()?;
        resp.write(format!("OTA update received, processing... {} bytes", nn).as_bytes())?;

        Result::<_, anyhow::Error>::Ok(())
    })?;

    server.fn_handler("/", Method::Get, |req| {
        req.into_ok_response()?
            .write_all(INDEX_HTML.as_bytes())
            .map(|_| ())
    })?;

    Ok(server)
}

const STACK_SIZE: usize = 10240;

fn create_server() -> anyhow::Result<EspHttpServer<'static>> {
    let server_configuration = esp_idf_svc::http::server::Configuration {
        stack_size: STACK_SIZE,
        ..Default::default()
    };

    Ok(EspHttpServer::new(&server_configuration)?)
}

mod lcd {
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
    static mut ESP_LCD_PANEL_HANDLE: esp_idf_svc::sys::esp_lcd_panel_handle_t =
        std::ptr::null_mut();
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
        buscfg.max_transfer_sz =
            (DISPLAY_WIDTH * DISPLAY_HEIGHT * std::mem::size_of::<u16>()) as i32;
        esp!(unsafe {
            spi_bus_initialize(SPI3::device(), &buscfg, spi_common_dma_t_SPI_DMA_CH_AUTO)
        })
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
        panel_config.__bindgen_anon_1.rgb_ele_order =
            lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_RGB;
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

    pub fn flush_display(
        color_data: &[u8],
        x_start: i32,
        y_start: i32,
        x_end: i32,
        y_end: i32,
    ) -> i32 {
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
        .set_vertical_offset(scroll_offset)
        .draw(display_target)?;

        // display_target.fix_background()?;

        display_target.flush()?;

        Ok(())
    }
}

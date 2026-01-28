use embedded_graphics::{
    framebuffer::{buffer_size, Framebuffer},
    pixelcolor::{raw::RawU1, BinaryColor},
    prelude::{Dimensions, DrawTarget, Point},
    text::Alignment,
};
use esp_idf_svc::hal::{
    gpio::{Gpio45, Gpio48, Pin},
    i2c::I2C0,
};

pub type Color = BinaryColor;

static mut BUS_HANDLE: esp_idf_svc::sys::i2c_master_bus_handle_t = std::ptr::null_mut();
static mut LCD_HANDLE_IO: esp_idf_svc::sys::esp_lcd_panel_io_handle_t = std::ptr::null_mut();
static mut LCD_HANDLE: esp_idf_svc::sys::esp_lcd_panel_handle_t = std::ptr::null_mut();

pub fn i2c_init(_i2c: I2C0, sda: Gpio48, scl: Gpio45) -> anyhow::Result<()> {
    use esp_idf_svc::hal::i2c::I2c;
    use esp_idf_svc::sys::*;

    let mut flags = i2c_master_bus_config_t__bindgen_ty_2::default();
    flags.set_enable_internal_pullup(1);

    let i2c_bus_config = i2c_master_bus_config_t {
        __bindgen_anon_1: i2c_master_bus_config_t__bindgen_ty_1 {
            clk_source: soc_periph_i2c_clk_src_t_I2C_CLK_SRC_DEFAULT,
        },
        i2c_port: I2C0::port() as i32,
        scl_io_num: scl.pin(),
        sda_io_num: sda.pin(),
        glitch_ignore_cnt: 7,
        flags,
        intr_priority: 0,
        trans_queue_depth: 0,
    };

    unsafe { esp!(i2c_new_master_bus(&i2c_bus_config, &raw mut BUS_HANDLE))? };
    Ok(())
}

pub fn init_i2c_lcd() -> anyhow::Result<()> {
    use esp_idf_svc::sys::*;

    unsafe {
        let mut flags = esp_lcd_panel_io_i2c_config_t__bindgen_ty_1::default();
        flags.set_disable_control_phase(0);
        flags.set_dc_low_on_data(0);

        let io_config = esp_lcd_panel_io_i2c_config_t {
            dev_addr: 0x3C,
            on_color_trans_done: None,
            user_ctx: std::ptr::null_mut(),
            control_phase_bytes: 1,
            dc_bit_offset: 6,
            lcd_cmd_bits: 8,
            lcd_param_bits: 8,
            flags,
            scl_speed_hz: 100_000,
        };

        esp!(esp_lcd_new_panel_io_i2c_v2(
            BUS_HANDLE,
            &io_config,
            &raw mut LCD_HANDLE_IO
        ))?;

        let mut ssd1306_config = esp_lcd_panel_ssd1306_config_t { height: 32 };

        let panel_dev_config = esp_lcd_panel_dev_config_t {
            reset_gpio_num: -1,
            bits_per_pixel: 1,
            vendor_config: &mut ssd1306_config as *mut _ as *mut std::ffi::c_void,
            ..Default::default()
        };

        esp!(esp_lcd_new_panel_ssd1306(
            LCD_HANDLE_IO,
            &panel_dev_config,
            &raw mut LCD_HANDLE,
        ))?;

        esp!(esp_lcd_panel_reset(LCD_HANDLE))?;
        esp!(esp_lcd_panel_init(LCD_HANDLE))?;
        esp!(esp_lcd_panel_invert_color(LCD_HANDLE, false))?;
        // esp!(esp_lcd_panel_swap_xy(LCD_HANDLE, true))?;
        esp!(esp_lcd_panel_mirror(LCD_HANDLE, true, true))?;
        esp!(esp_lcd_panel_disp_on_off(LCD_HANDLE, true))?;
    }

    Ok(())
}

// pub type FramebufferType = Framebuffer<
//     BinaryColor,
//     RawU1,
//     embedded_graphics::pixelcolor::raw::LittleEndian,
//     128,
//     32,
//     { buffer_size::<BinaryColor>(128, 32) },
// >;

// pub fn new_lcd_text_buffer() -> Box<FramebufferType> {
//     Box::new(Framebuffer::<
//         BinaryColor,
//         RawU1,
//         embedded_graphics::pixelcolor::raw::LittleEndian,
//         128,
//         32,
//         { buffer_size::<BinaryColor>(128, 32) },
//     >::new())
// }

pub type FramebufferType = SSD1306Framebuffer128x32;

pub fn new_lcd_text_buffer() -> Box<FramebufferType> {
    Box::new(SSD1306Framebuffer128x32 {
        data: [0u8; 128 * 32 / 8],
    })
}

pub fn lcd_display_bitmap(buffer: &FramebufferType) -> anyhow::Result<()> {
    use esp_idf_svc::sys::*;

    unsafe {
        esp!(esp_lcd_panel_draw_bitmap(
            LCD_HANDLE,
            0,
            0,
            128,
            32,
            buffer.data().as_ptr() as *const _
        ))?;
    }

    Ok(())
}

pub fn lcd_display_text(buffer: &mut FramebufferType, text: &str) -> anyhow::Result<()> {
    use esp_idf_svc::sys::*;

    use embedded_graphics::mono_font::jis_x0201::FONT_8X13;
    use embedded_graphics::mono_font::MonoTextStyle;
    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::Drawable;

    const STYLE: MonoTextStyle<'_, BinaryColor> = MonoTextStyle::new(&FONT_8X13, BinaryColor::On);

    embedded_graphics::text::Text::with_alignment(
        text,
        buffer.bounding_box().center(),
        STYLE,
        Alignment::Center,
    )
    .draw(buffer)?;

    unsafe {
        esp!(esp_lcd_panel_draw_bitmap(
            LCD_HANDLE,
            0,
            0,
            128,
            32,
            buffer.data().as_ptr() as *const _
        ))?;
    }

    Ok(())
}

pub fn lcd_display_test() -> anyhow::Result<()> {
    use esp_idf_svc::sys::*;

    unsafe {
        let buffer: [u8; 1] = [0b00000001];

        esp!(esp_lcd_panel_draw_bitmap(
            LCD_HANDLE,
            0,
            0,
            1,
            8,
            buffer.as_ptr() as *const std::ffi::c_void
        ))?;
    }

    Ok(())
}

pub struct SSD1306Framebuffer128x32 {
    data: [u8; 128 * 32 / 8],
}

impl SSD1306Framebuffer128x32 {
    pub fn clear(&mut self, color: BinaryColor) -> Result<(), anyhow::Error> {
        let fill_byte = match color {
            BinaryColor::On => 0xFF,
            BinaryColor::Off => 0x00,
        };
        for byte in self.data.iter_mut() {
            *byte = fill_byte;
        }
        Ok(())
    }

    pub fn set_pixel(&mut self, point: Point, color: BinaryColor) {
        if point.x < 0 || point.x >= 128 || point.y < 0 || point.y >= 32 {
            return;
        }
        let x = point.x as usize;
        let y = point.y as usize;

        let bit_index = y % 8;
        let byte_index = x + (y / 8) * 128;

        match color {
            BinaryColor::On => {
                self.data[byte_index] |= 1 << bit_index;
            }
            BinaryColor::Off => {
                self.data[byte_index] &= !(1 << bit_index);
            }
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl embedded_graphics::prelude::OriginDimensions for SSD1306Framebuffer128x32 {
    fn size(&self) -> embedded_graphics::prelude::Size {
        embedded_graphics::prelude::Size::new(128, 32)
    }
}

impl DrawTarget for SSD1306Framebuffer128x32 {
    type Color = BinaryColor;

    type Error = anyhow::Error;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
    {
        for p in pixels {
            let embedded_graphics::Pixel(point, color) = p;
            if point.x < 0 || point.x >= 128 || point.y < 0 || point.y >= 32 {
                continue;
            }
            let x = point.x as usize;
            let y = point.y as usize;

            let bit_index = y % 8;
            let byte_index = x + (y / 8) * 128;

            match color {
                BinaryColor::On => {
                    self.data[byte_index] |= 1 << bit_index;
                }
                BinaryColor::Off => {
                    self.data[byte_index] &= !(1 << bit_index);
                }
            }
        }
        Ok(())
    }
}

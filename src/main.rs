use embedded_graphics::prelude::RgbColor;

use crate::lcd::DisplayTargetDrive;

mod ansi_plugin;
mod app;
mod audio;
mod bt_keyboard;
mod crab_img;
mod i2c;
mod lcd;
mod wifi;
mod ws;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;
    let partition = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;

    let mut bl = esp_idf_svc::hal::gpio::PinDriver::output(peripherals.pins.gpio11)?;
    bl.set_low()?;

    // let mut backlight = lcd::backlight_init(peripherals.pins.gpio11.into())?;
    // lcd::set_backlight(&mut backlight, 40).unwrap();

    log_heap();

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

    let mut target = lcd::FrameBuffer::new(lcd::ColorFormat::BLACK);
    target.flush()?;

    std::thread::sleep(std::time::Duration::from_secs(2));

    log::info!("Displaying PNG image on LCD...");

    lcd::display_png(
        &mut target,
        lcd::DEFAULT_BACKGROUND,
        std::time::Duration::from_secs(2),
    )?;
    lcd::display_text(&mut target, "VibeKeys Ready")?;

    let mut wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;
    let ssid = std::env!("SSID");
    let password = std::env!("PASSWORD");
    wifi::connect(&mut wifi, ssid, password, sysloop.clone())?;

    let (tx, rx) = tokio::sync::mpsc::channel::<app::Event>(64);

    let worker = audio::AudioWorker {
        in_i2s: peripherals.i2s0,
        in_ws: peripherals.pins.gpio45.into(),
        in_clk: peripherals.pins.gpio48.into(),
        din: peripherals.pins.gpio41.into(),
        in_mclk: None,
    };

    const AUDIO_STACK_SIZE: usize = 15 * 1024;
    let mac = wifi.sta_netif().get_mac().unwrap();
    let dev_id = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let r = std::thread::Builder::new()
        .stack_size(AUDIO_STACK_SIZE)
        .spawn(move || {
            log::info!(
                "Starting audio worker thread in core {:?}",
                esp_idf_svc::hal::cpu::core()
            );
            let r = worker.run(tx);
            if let Err(e) = r {
                log::error!("Audio worker error: {:?}", e);
            }
        })
        .map_err(|e| anyhow::anyhow!("Failed to spawn audio worker thread: {:?}", e))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut btn6 = esp_idf_svc::hal::gpio::PinDriver::input(peripherals.pins.gpio6)?;
    btn6.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    btn6.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    runtime.spawn(async move {
        loop {
            if let Err(e) = btn6.wait_for_falling_edge().await {
                log::error!("Button interrupt error: {:?}", e);
                continue;
            }

            let r = audio::MIC_ON.fetch_not(std::sync::atomic::Ordering::Relaxed);
            log::info!("Button pressed, mic state changed to: {}", !r);

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let app_fut = app::run(format!("ws://192.168.1.28:8080/ws/{}", dev_id), rx);
    let r = runtime.block_on(app_fut);
    if let Err(e) = r {
        log::error!("App error: {:?}", e);
    } else {
        log::info!("App exited successfully");
    }

    Ok(())
}

pub fn handle_key_event(
    keyboard: &mut bt_keyboard::KeyboardAndMouse,
    code: u8,
    pressed: bool,
) -> anyhow::Result<()> {
    if pressed {
        match code {
            0 => {
                keyboard.write("/compact\n");
            }
            1 => {}
            2 => {
                keyboard.press(b'\t');
            }
            3 => {
                keyboard.press(0x1b); // ESC
            }
            4 => {
                keyboard.write("retry\n");
            }
            5 => {
                keyboard.shift_press(b'\t');
            }
            6 => {
                keyboard.r_ctrl_press(0);
            }
            7 => {
                keyboard.press(b'\n'); // Enter
            }
            18 => {}
            _ => return Ok(()),
        };
    } else {
        keyboard.release();
    }

    Ok(())
}

pub fn log_heap() {
    unsafe {
        use esp_idf_svc::sys::{heap_caps_get_free_size, MALLOC_CAP_INTERNAL, MALLOC_CAP_SPIRAM};

        log::info!(
            "Free SPIRAM heap size: {}KB",
            heap_caps_get_free_size(MALLOC_CAP_SPIRAM) / 1024
        );
        log::info!(
            "Free INTERNAL heap size: {}KB",
            heap_caps_get_free_size(MALLOC_CAP_INTERNAL) / 1024
        );
    }
}

use std::sync::{Arc, Mutex};

use embedded_graphics::prelude::RgbColor;
use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver};

use crate::lcd::DisplayTargetDrive;

mod ansi_plugin;
mod app;
mod audio;
mod bt_keyboard_mode;
mod bt_wifi_mode;
mod i2c;
mod lcd;
mod protocol;
mod wifi;
mod ws;

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

    let btn0 = new_btn(
        peripherals.pins.gpio0.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

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

    let mut target = lcd::FrameBuffer::new(lcd::ColorFormat::WHITE);
    target.flush()?;
    lcd::display_text(&mut target, "VibeKeys Starting...\n Read setting", 0)?;

    let mut wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;
    let mac = wifi.sta_netif().get_mac().unwrap();
    let dev_id = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let btn4 = new_btn(
        peripherals.pins.gpio4.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    let nvs = esp_idf_svc::nvs::EspDefaultNvs::new(partition, "setting", true)?;

    let mut setting = bt_wifi_mode::Setting::load_from_nvs(&nvs)?;

    if btn4.is_low() || setting.need_init() {
        esp32_nimble::BLEDevice::set_device_name("VibeKeys-MAX")?;
        setting.background_png.0.clear();

        let (tx, rx) = std::sync::mpsc::channel();
        let setting_arc = Arc::new(Mutex::new((setting, nvs)));
        lcd::display_text(&mut target, "Setting Mode", 0)?;
        bt_wifi_mode::bt(&dev_id, setting_arc.clone(), tx)?;

        match rx.recv() {
            Ok(bt_wifi_mode::BTevent::Reset) => {
                let mut lock = setting_arc.lock().unwrap();

                {
                    let (png, b) = &mut lock.0.background_png;
                    if *b {
                        lcd::display_png(&mut target, png, std::time::Duration::from_secs(3))
                            .unwrap();
                        let png = std::mem::take(png);
                        if let Err(_) = lock.1.set_blob("background_png", &png) {
                            lcd::display_text(
                                &mut target,
                                &format!("Failed to save background PNG"),
                                0,
                            )
                            .unwrap();
                        }
                    }
                }
                for i in 1..=3 {
                    lcd::display_text(
                        &mut target,
                        &format!("Received Setting from BLE\n SSID:{}\n SERVER_URL:{}\n Restarting in {}s", lock.0.ssid, lock.0.server_url, i),
                        0,
                    )?;
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                unsafe {
                    esp_idf_svc::sys::esp_restart();
                }
            }
            Ok(bt_wifi_mode::BTevent::GoToOta) => {
                for i in 1..=5 {
                    lcd::display_text(
                        &mut target,
                        &format!("OTA is not yet supported.\n Restarting in {}s", i),
                        0,
                    )?;
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            Err(e) => {
                log::error!("Error receiving BLE event: {:?}", e);
                for i in (1..=5).rev() {
                    lcd::display_text(
                        &mut target,
                        &format!("Error receiving from BLE\n Restarting in {}s", i),
                        0,
                    )?;
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }

                unsafe {
                    esp_idf_svc::sys::esp_restart();
                }
            }
        }

        unsafe {
            esp_idf_svc::sys::esp_restart();
        }
    }

    log::info!("Displaying PNG image on LCD...");

    if setting.background_png.0.is_empty() {
        log::info!("No background PNG found in settings, using default.");
        std::thread::sleep(std::time::Duration::from_secs(2));
    } else {
        log::info!(
            "Background PNG found in settings, size: {} bytes",
            setting.background_png.0.len()
        );
        lcd::display_png(
            &mut target,
            setting.background_png.0.as_slice(),
            std::time::Duration::from_secs(2),
        )?;
    }

    lcd::display_text(&mut target, "Connecting the server and WiFi...", 0)?;

    let r = wifi::connect(&mut wifi, &setting.ssid, &setting.pass, sysloop.clone());
    if r.is_err() {
        log::error!("Failed to connect to WiFi: {:?}", r.err());
        lcd::display_text(&mut target, " WiFi connection failed\n", 0)?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        unsafe {
            esp_idf_svc::sys::esp_restart();
        }
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<app::Event>(64);

    let worker = audio::AudioWorker {
        in_i2s: peripherals.i2s0,
        in_ws: peripherals.pins.gpio41.into(),
        in_clk: peripherals.pins.gpio42.into(),
        din: peripherals.pins.gpio40.into(),
        in_mclk: None,
    };

    const AUDIO_STACK_SIZE: usize = 15 * 1024;

    let audio_tx = tx.clone();
    let _ = std::thread::Builder::new()
        .stack_size(AUDIO_STACK_SIZE)
        .spawn(move || {
            log::info!(
                "Starting audio worker thread in core {:?}",
                esp_idf_svc::hal::cpu::core()
            );
            let r = worker.run(audio_tx);
            if let Err(e) = r {
                log::error!("Audio worker error: {:?}", e);
            }
        })
        .map_err(|e| anyhow::anyhow!("Failed to spawn audio worker thread: {:?}", e))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    {
        runtime.spawn(app::key_task::mic_key(btn0, setting.mic_model.into()));

        let btn2 = new_btn(
            peripherals.pins.gpio2.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::listen_key_event(
            btn2,
            tx.clone(),
            app::Event::UltraThink,
        ));

        runtime.spawn(app::key_task::listen_key_event(
            btn4,
            tx.clone(),
            app::Event::GUI,
        ));

        let btn5 = new_btn(
            peripherals.pins.gpio5.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::listen_key_event(
            btn5,
            tx.clone(),
            app::Event::SwtchMode,
        ));

        let btn6 = new_btn(
            peripherals.pins.gpio6.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;
        runtime.spawn(app::key_task::backspace_key(btn6, tx.clone()));

        let btn3 = new_btn(
            peripherals.pins.gpio3.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::esc_key(btn3, tx.clone()));

        let btn7 = new_btn(
            peripherals.pins.gpio7.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::accept_key(btn7, tx.clone()));

        let pin16 = new_btn(
            peripherals.pins.gpio16.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;
        let pin17 = new_btn(
            peripherals.pins.gpio17.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::rotate_key(pin16, pin17, tx.clone()));

        let pin18 = new_btn(
            peripherals.pins.gpio18.into(),
            esp_idf_svc::hal::gpio::Pull::Up,
            esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
        )?;

        runtime.spawn(app::key_task::rotate_push_key(pin18, tx.clone()));
    }

    let mut ui = lcd::UI::new_with_target(target);

    let app_fut = app::run(setting.server_url, &mut ui, rx);
    let r = runtime.block_on(app_fut);
    if let Err(e) = r {
        log::error!("App error: {:?}", e);
    } else {
        log::info!("App exited successfully");
    }

    unsafe {
        esp_idf_svc::sys::esp_restart();
    }
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

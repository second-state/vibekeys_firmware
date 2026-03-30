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

const DEFAULT_SNTP_SERVERS: [&str; 4] = [
    "pool.ntp.org",
    "time.apple.com",
    "time.windows.com",
    "time.google.com",
];

pub fn sync_time(display_target: &mut lcd::FrameBuffer) -> anyhow::Result<()> {
    use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncMode, SyncStatus};

    for i in 0..DEFAULT_SNTP_SERVERS.len() {
        log::info!("SNTP sync time with server: {}", DEFAULT_SNTP_SERVERS[i]);
        lcd::display_text(
            display_target,
            &format!("Syncing time with {}", DEFAULT_SNTP_SERVERS[i]),
            0,
        )?;

        let conf = SntpConf {
            servers: [DEFAULT_SNTP_SERVERS[i]],
            operating_mode: OperatingMode::Poll,
            sync_mode: SyncMode::Immediate,
        };
        let ntp_client = EspSntp::new(&conf)?;

        for _ in 0..30 {
            let status = ntp_client.get_sync_status();
            log::info!("sntp sync status {:?}", status);
            if status == SyncStatus::Completed {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        log::info!("SNTP synchronized!");
    }

    Err(anyhow::anyhow!("Failed to sync time with all SNTP servers"))
}

pub fn goto_next_firmware() -> anyhow::Result<()> {
    use esp_idf_svc::sys::{esp_ota_get_next_update_partition, esp_ota_set_boot_partition};

    unsafe {
        let partition = esp_ota_get_next_update_partition(std::ptr::null());
        esp_idf_svc::sys::esp!(esp_ota_set_boot_partition(partition))?;
    };

    esp_idf_svc::hal::reset::restart();
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

    // MIC
    let btn0 = new_btn(
        peripherals.pins.gpio0.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // GUI (claude)
    let mut btn4 = new_btn(
        peripherals.pins.gpio4.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // ESC
    let mut btn3 = new_btn(
        peripherals.pins.gpio3.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // UltraThink
    let btn2 = new_btn(
        peripherals.pins.gpio2.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Switch Mode
    let btn5 = new_btn(
        peripherals.pins.gpio5.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Backspace
    let btn6 = new_btn(
        peripherals.pins.gpio6.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Accept
    let mut btn7 = new_btn(
        peripherals.pins.gpio7.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Rotate A
    let pin16 = new_btn(
        peripherals.pins.gpio16.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Rotate B
    let pin17 = new_btn(
        peripherals.pins.gpio17.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Rotate Push
    let pin18 = new_btn(
        peripherals.pins.gpio18.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    let nvs = esp_idf_svc::nvs::EspDefaultNvs::new(partition, "setting", true)?;

    let mut setting = bt_wifi_mode::Setting::load_from_nvs(&nvs)?;

    let mut wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;
    let mac = wifi.sta_netif().get_mac().unwrap();
    let dev_id = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut mode = 3;

    for i in 0..5 {
        lcd::display_text(&mut target, format!(" <ESC> -> OTA mode\n <Claude> -> Setting mode\n <Accept> -> Remote Control mode\n{}s later enter Keyboard mode", 5-i).as_str(), 0).unwrap();

        mode = runtime.block_on(async {
            tokio::select! {
                _ = btn3.wait_for_low() => {
                    log::info!("Button ESC is pressed, Goto ota mode");
                    0
                },
                _ = btn7.wait_for_low() => {
                    log::info!("Button Accept is pressed, Starting in Remote Control mode");
                    1
                },
                _ = btn4.wait_for_low() => {
                    log::info!("Button Setting is pressed, Starting in setting mode");
                    2
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    log::info!("No button is pressed, Starting in normal mode");
                    3
                }
            }
        });
        if mode != 3 {
            break;
        }
    }

    if mode == 0 {
        log::info!("Button ESC is pressed, Goto ota mode");
        lcd::display_text(&mut target, "Entering OTA mode...", 0)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        goto_next_firmware()?;
    } else {
        let mut ota = esp_idf_svc::ota::EspOta::new()?;
        ota.mark_running_slot_valid()?;
    }

    if mode == 3 {
        lcd::display_text(&mut target, "Starting in keyboard mode...", 0)?;
        std::thread::sleep(std::time::Duration::from_secs(1));

        let (tx, mut rx) = tokio::sync::mpsc::channel(64);

        esp32_nimble::BLEDevice::set_device_name("VibeKeys-MAX")?;

        let ble_device = esp32_nimble::BLEDevice::take();
        let mut keyboard = bt_keyboard_mode::KeyboardAndMouse::new(ble_device, 100)?;
        let _controller_service = bt_keyboard_mode::new_controller_service(ble_device, tx)?;

        let server = ble_device.get_server();
        server.start()?;
        bt_keyboard_mode::start_ble_advertising(ble_device, keyboard.hid_service_id())?;

        let mut key_pins = bt_keyboard_mode::KeysPin {
            mic: btn0,
            ultrathink: btn2,
            esc: btn3,
            gui: btn4,
            switch: btn5,
            backspace: btn6,
            accept: btn7,
            rotate_a: pin16,
            rotate_b: pin17,
            rotate_button: pin18,
        };

        lcd::display_text(&mut target, "Keyboard Mode", 0)?;

        keyboard_mode_main(
            &runtime,
            &mut target,
            ble_device,
            &mut keyboard,
            &mut key_pins,
            &mut rx,
        );
    }

    if mode == 2 || setting.need_init() {
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
                esp_idf_svc::hal::reset::restart();
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

                esp_idf_svc::hal::reset::restart();
            }
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

    let (tx, rx) = tokio::sync::mpsc::channel::<app::Event>(64);

    {
        runtime.spawn(app::key_task::mic_key(btn0, setting.mic_model.into()));

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

        runtime.spawn(app::key_task::listen_key_event(
            btn5,
            tx.clone(),
            app::Event::SwtchMode,
        ));

        runtime.spawn(app::key_task::backspace_key(btn6, tx.clone()));

        runtime.spawn(app::key_task::esc_key(btn3, tx.clone()));

        runtime.spawn(app::key_task::accept_key(btn7, tx.clone()));

        runtime.spawn(app::key_task::rotate_key(pin16, pin17, tx.clone()));

        runtime.spawn(app::key_task::rotate_push_key(pin18, tx.clone()));
    }

    lcd::display_text(&mut target, "Connecting the WiFi...", 0)?;

    let r = wifi::connect(&mut wifi, &setting.ssid, &setting.pass, sysloop.clone());
    if r.is_err() {
        log::error!("Failed to connect to WiFi: {:?}", r.err());
        lcd::display_text(&mut target, " WiFi connection failed\n", 0)?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        unsafe {
            esp_idf_svc::sys::esp_restart();
        }
    }

    if setting.server_url.starts_with("wss") {
        _ = rustls_rustcrypto::provider().install_default();
        lcd::display_text(&mut target, "Syncing time...", 0)?;
        let r = sync_time(&mut target);
        if r.is_err() {
            log::error!("Failed to sync time: {:?}", r.err());
            lcd::display_text(&mut target, " Time sync failed\n", 0)?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            unsafe {
                esp_idf_svc::sys::esp_restart();
            }
        }
    }

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

    lcd::display_text(&mut target, "Connecting the Server...", 0)?;

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

fn keyboard_mode_main(
    runtime: &tokio::runtime::Runtime,
    display: &mut lcd::FrameBuffer,
    ble_device: &mut esp32_nimble::BLEDevice,
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    key_pins: &mut bt_keyboard_mode::KeysPin,
    rx: &mut tokio::sync::mpsc::Receiver<bt_keyboard_mode::ControllerCommand>,
) -> ! {
    loop {
        let event = runtime.block_on(bt_keyboard_mode::key_event(key_pins, rx));
        let _ = handle_key_event(display, ble_device, keyboard, event);
    }
}

pub fn handle_key_event(
    display: &mut lcd::FrameBuffer,
    ble_device: &mut esp32_nimble::BLEDevice,
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    event: bt_keyboard_mode::ControllerCommand,
) -> anyhow::Result<()> {
    log::info!("Handling controller command: {:?}", event);
    use bt_keyboard_mode::KeysPin;
    match event {
        bt_keyboard_mode::ControllerCommand::DisplayKeyboard(text) => {
            lcd::display_text(display, &text, 0)?;
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::MIC) => {
            keyboard.press(b' '); // Space for mic on/off toggle
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::MIC) => {
            keyboard.release(); // Release space
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::ULTRATHINK) => {
            keyboard.write("ultrathink ");
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::ULTRATHINK) => {}
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::ESC) => {
            keyboard.press(0x1b); // ESC
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::ESC) => {
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::GUI) => {
            keyboard.write("claude");
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::GUI) => {}
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::SWITCH) => {
            keyboard.shift_press(b'\t'); // Shift + Tab for switch mode
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::SWITCH) => {
            keyboard.release(); // Release Shift + Tab
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::BACKSPACE) => {
            keyboard.press(0x08); // Backspace
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::BACKSPACE) => {
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::ACCEPT) => {
            {
                let mut adv = ble_device.get_advertising().lock();
                let adv_is_advertising = adv.is_advertising();
                log::info!("Checking advertising state... {}", adv_is_advertising);
                if !adv_is_advertising {
                    adv.start().unwrap();
                }
            }
            keyboard.press(b'\n'); //
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::ACCEPT) => {
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(KeysPin::ROTATE_BUTTON) => {
            keyboard.press(b' ');
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(KeysPin::ROTATE_BUTTON) => {
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::RotateDown => {
            // 箭头下
            keyboard.press_raw(0x51, 0); // HID Down Arrow
            std::thread::sleep(std::time::Duration::from_millis(200));
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::RotateUp => {
            // 箭头上
            keyboard.press_raw(0x52, 0); // HID Up Arrow
            std::thread::sleep(std::time::Duration::from_millis(200));
            keyboard.release();
        }
        _ => {}
    }

    Ok(())
}

use std::sync::{Arc, Mutex};

use embedded_graphics::prelude::WebColors;
use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver};

use crate::lcd::DisplayTargetDrive;

mod ansi_plugin;
mod app;
mod audio;
mod bt_keyboard_mode;
mod bt_wifi_mode;
#[cfg(feature = "i2c_oled")]
mod i2c;
mod lcd;
mod protocol;
mod util;
mod wifi;
mod ws;

type AnyBtn = PinDriver<'static, esp_idf_svc::hal::gpio::Input>;

fn new_btn(
    pin: AnyIOPin<'static>,
    pull: esp_idf_svc::hal::gpio::Pull,
    interrupt: esp_idf_svc::hal::gpio::InterruptType,
) -> anyhow::Result<AnyBtn> {
    let mut btn = PinDriver::input(pin, pull)?;
    btn.set_interrupt_type(interrupt)?;
    Ok(btn)
}

const DEFAULT_SNTP_SERVERS: [&str; 4] = [
    "time.apple.com",
    "time.windows.com",
    "time.google.com",
    "pool.ntp.org",
];

pub fn sync_time(display_target: &mut lcd::FrameBuffer) -> anyhow::Result<()> {
    use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncMode, SyncStatus};

    for i in 0..DEFAULT_SNTP_SERVERS.len() {
        log_heap();
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

        for _ in 0..15 {
            let status = ntp_client.get_sync_status();
            log::info!("sntp sync status {:?}", status);
            log_heap();
            if status == SyncStatus::Completed {
                lcd::display_text(display_target, "Syncing time Completed", 0)?;
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
    let peripherals = esp_idf_svc::hal::peripherals::Peripherals::take().unwrap();
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;
    let partition = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;

    let mut bl = esp_idf_svc::hal::gpio::PinDriver::output(peripherals.pins.gpio11)?;
    if cfg!(feature = "max2") {
        bl.set_high()?;
    } else {
        bl.set_low()?;
    }

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

    let mut target = lcd::FrameBuffer::new(lcd::ColorFormat::CSS_BLACK);
    target.flush()?;
    lcd::display_text(&mut target, "VibeKeys Starting...\n Read setting", 0)?;

    // MIC
    let btn0 = new_btn(
        peripherals.pins.gpio0.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // NEXT
    let btn4 = new_btn(
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

    // Custom
    let btn2 = new_btn(
        peripherals.pins.gpio2.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Backspace
    let btn5 = new_btn(
        peripherals.pins.gpio5.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // YOLO
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

    let mut nvs = esp_idf_svc::nvs::EspDefaultNvs::new(partition, "setting", true)?;

    if btn3.is_low() {
        lcd::display_text(&mut target, "Clear all config", 0)?;
        bt_wifi_mode::Setting::clear_nvs(&mut nvs)?;
        bt_keyboard_mode::KeymapConfig::clear_nvs(&mut nvs)?;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let setting = bt_wifi_mode::Setting::load_from_nvs(&nvs)?;
    // Load keymap config before moving nvs
    let mut keymap = bt_keyboard_mode::KeymapConfig::load_from_nvs(&nvs)?;
    log::info!("Loaded keymap config with {} keys", keymap.keys.len());
    let asr_config = audio::AsrConfig::load_from_nvs(&nvs);

    let mut wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;
    let mac = wifi.sta_netif().get_mac().unwrap();
    let _dev_id = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    if let Err(e) = runtime {
        log::error!("Failed to create Tokio runtime: {:?}", e);
        lcd::display_text(
            &mut target,
            &format!("Failed to create Tokio runtime:\n{:?}", e),
            0,
        )?;
        std::thread::sleep(std::time::Duration::from_secs(5));
        esp_idf_svc::hal::reset::restart();
    }

    let runtime = runtime.unwrap();

    let mut mode = 3;

    for i in 0..5 {
        lcd::display_text(&mut target, format!(" <ESC> -> OTA mode\n <Accept> -> Remote Control mode\n{}s later enter Keyboard mode", 5-i).as_str(), 0).unwrap();

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

    if mode == 1 && setting.need_init() {
        lcd::display_text(
            &mut target,
            "Remote Control mode requires network/server config",
            0,
        )?;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    if mode == 3 || setting.need_init() {
        lcd::display_text(&mut target, "Starting in keyboard mode...", 0)?;
        std::thread::sleep(std::time::Duration::from_secs(1));

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let (setting_tx, setting_rx) = tokio::sync::mpsc::channel(8);

        let mut setting_arc = Arc::new(Mutex::new((setting.clone(), nvs)));

        esp32_nimble::BLEDevice::set_device_name("VibeKeys-MAX")?;

        let ble_device = esp32_nimble::BLEDevice::take();

        let adv = ble_device.get_advertising();

        let server = ble_device.get_server();
        server.on_connect(|server, desc| {
            log::info!("Client connected: {:?}", desc);
            if server.connected_count() < 5 {
                log::info!("Starting advertising for next client");
                if let Err(e) = adv.lock().start() {
                    log::error!("Failed to start advertising: {:?}", e);
                }
            } else {
                log::info!("Max clients connected, not advertising");
            }
        });
        let service = server.create_service(bt_wifi_mode::SERVICE_ID);

        let mut keyboard = bt_keyboard_mode::KeyboardAndMouse::new(ble_device, 100)?;
        let (controller, service_id) = {
            let mut lock = service.lock();
            let controller = bt_keyboard_mode::new_controller_service(&mut lock, tx)?;
            // Start setting service
            bt_wifi_mode::new_setting_service(&mut lock, setting_arc.clone(), Some(setting_tx))?;
            (controller, lock.uuid())
        };

        let server = ble_device.get_server();
        server.start()?;
        bt_keyboard_mode::start_ble_advertising(
            ble_device,
            &[keyboard.hid_service_id(), service_id],
        )?;

        let mut key_pins = bt_keyboard_mode::KeysPin {
            mic: btn0,
            custom: btn2,
            esc: btn3,
            next: btn4,
            backspace: btn5,
            switch: btn6,
            accept: btn7,
            rotate_a: pin16,
            rotate_b: pin17,
            rotate_button: pin18,
        };

        let mut driver: Option<audio::Driver> = None;

        let r = wifi::connect(&mut wifi, &setting.ssid, &setting.pass, sysloop.clone());
        if r.is_err() {
            let e = r.err();
            log::error!("Failed to connect to WiFi: {:?}", e);
            lcd::display_text(
                &mut target,
                &format!(" WiFi connection failed: {:?}\n", e),
                0,
            )?;
            std::thread::sleep(std::time::Duration::from_secs(3));
        } else {
            log::info!("WiFi connected successfully");
            log::info!("ASR config loaded from NVS: {:?}", asr_config);

            if let Some(ref asr_config) = asr_config {
                if asr_config.requires_tls() {
                    let r = sync_time(&mut target);
                    if r.is_err() {
                        log::error!("Failed to sync time: {:?}", r.err());
                        let _ = lcd::display_text(&mut target, " Time sync failed\n", 0);
                        std::thread::sleep(std::time::Duration::from_secs(3));
                    } else {
                        let worker = audio::AudioWorker {
                            in_i2s: peripherals.i2s0,
                            in_ws: peripherals.pins.gpio41.into(),
                            in_clk: peripherals.pins.gpio42.into(),
                            din: peripherals.pins.gpio40.into(),
                            in_mclk: None,
                        };
                        let _ = driver.insert(audio::Driver::new(worker)?);
                    }
                } else {
                    let worker = audio::AudioWorker {
                        in_i2s: peripherals.i2s0,
                        in_ws: peripherals.pins.gpio41.into(),
                        in_clk: peripherals.pins.gpio42.into(),
                        din: peripherals.pins.gpio40.into(),
                        in_mclk: None,
                    };
                    let _ = driver.insert(audio::Driver::new(worker)?);
                }
            }
        }

        log_heap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        lcd::display_text(&mut target, "Keyboard Mode", 0)?;

        runtime.block_on(keyboard_mode_main(
            &mut target,
            ble_device,
            &mut keyboard,
            &mut key_pins,
            &mut setting_arc,
            setting_rx,
            rx,
            &mut keymap,
            driver,
            asr_config,
            controller,
        ));
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
            app::Event::Custom,
        ));

        runtime.spawn(app::key_task::listen_key_event(
            btn4,
            tx.clone(),
            app::Event::NEXT,
        ));

        runtime.spawn(app::key_task::backspace_key(btn5, tx.clone()));

        runtime.spawn(app::key_task::listen_key_event(
            btn6,
            tx.clone(),
            app::Event::SwitchMode,
        ));

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
        esp_idf_svc::hal::reset::restart();
    }

    if setting.server_url.starts_with("wss") {
        // _ = rustls_rustcrypto::provider().install_default();
        lcd::display_text(&mut target, "Syncing time...", 0)?;
        let r = sync_time(&mut target);
        if r.is_err() {
            log::error!("Failed to sync time: {:?}", r.err());
            lcd::display_text(&mut target, " Time sync failed\n", 0)?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            esp_idf_svc::hal::reset::restart();
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

    let app_fut = app::run(setting.server_url, &mut ui, rx, &keymap);
    let r = runtime.block_on(app_fut);
    if let Err(e) = r {
        log::error!("App error: {:?}", e);
    } else {
        log::info!("App exited successfully");
    }

    esp_idf_svc::hal::reset::restart();
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

fn handle_reset_event(
    setting_arc: &mut Arc<Mutex<(bt_wifi_mode::Setting, esp_idf_svc::nvs::EspDefaultNvs)>>,
) -> ! {
    let mut lock = setting_arc.lock().unwrap();
    let png_to_save = if lock.0.background_png.1 {
        Some(lock.0.background_png.0.clone())
    } else {
        None
    };

    if let Some(png) = png_to_save {
        if lock.1.set_blob("background_png", &png).is_err() {
            log::error!("Failed to save background PNG");
        }
    }

    log::info!(
        "Received Reset from BLE, SSID:{}, SERVER_URL:{}, restarting",
        lock.0.ssid,
        lock.0.server_url
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    esp_idf_svc::hal::reset::restart();
}

fn handle_keymap_config(
    config: String,
    nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
    keymap: &mut bt_keyboard_mode::KeymapConfig,
) {
    log::info!("Received keymap config: {}", config);
    match bt_keyboard_mode::KeymapConfig::from_json(&config) {
        Ok(keymap_) => {
            keymap.merge(keymap_);
            match keymap.save_to_nvs(nvs) {
                Ok(()) => {
                    log::info!("Keymap config merged and saved to NVS successfully");
                }
                Err(e) => {
                    log::error!("Failed to save keymap to NVS: {:?}", e);
                }
            }
        }
        Err(e) => {
            log::error!("Failed to parse keymap JSON: {:?}", e);
        }
    }
}

fn handle_keymap_asr_config(
    config: String,
    nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
) -> anyhow::Result<String> {
    log::info!("Received keymap config: {}", config);
    if config.is_empty() {
        nvs.remove("asr_config").ok();
        return Ok("ASR config cleared".to_string());
    }
    match audio::AsrConfig::from_json(&config) {
        Ok(config) => match config.save_to_nvs(nvs) {
            Ok(()) => {
                log::info!("asr config merged and saved to NVS successfully");
                Ok(format!(
                    "ASR config updated: {}",
                    serde_json::to_string_pretty(&config)
                        .unwrap_or_else(|_| "Failed to serialize ASR config".to_string())
                ))
            }
            Err(e) => {
                anyhow::bail!("Failed to save asr_config to NVS: {:?}", e);
            }
        },
        Err(e) => {
            anyhow::bail!("Failed to parse asr_config JSON: {:?}", e);
        }
    }
}

async fn keyboard_mode_main(
    display: &mut lcd::FrameBuffer,
    ble_device: &mut esp32_nimble::BLEDevice,
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    key_pins: &mut bt_keyboard_mode::KeysPin,
    setting_arc: &mut Arc<Mutex<(bt_wifi_mode::Setting, esp_idf_svc::nvs::EspDefaultNvs)>>,
    mut setting_rx: tokio::sync::mpsc::Receiver<bt_wifi_mode::BTevent>,
    mut rx: tokio::sync::mpsc::Receiver<bt_keyboard_mode::ControllerCommand>,
    keymap: &mut bt_keyboard_mode::KeymapConfig,
    mut driver: Option<audio::Driver>,
    asr_config: Option<audio::AsrConfig>,
    controller: bt_keyboard_mode::ControllerService,
) -> ! {
    loop {
        let event = tokio::select! {
            // Handle setting events (e.g., reset)
            Some(bt_wifi_mode::BTevent::Reset) = setting_rx.recv() => {
                handle_reset_event(setting_arc);
            }
            // Handle physical key events
            key_evt = bt_keyboard_mode::wait_key_event(key_pins) => key_evt,
            // Handle controller commands from BLE
            Some(evt) = rx.recv() => {
                match evt {
                    bt_keyboard_mode::ControllerCommand::KeymapConfig(config) => {
                        handle_keymap_config(
                            config,
                            &mut setting_arc.lock().unwrap().1,
                            keymap,
                        );
                        lcd::display_text(display, "keymap updated!", 0).unwrap();
                        continue;
                    }
                    bt_keyboard_mode::ControllerCommand::AsrConfig(config) => {
                        match handle_keymap_asr_config(config, &mut setting_arc.lock().unwrap().1) {
                            Ok(msg) => {
                                lcd::display_text(display, &msg, 0).unwrap();
                            }
                            Err(e) => {
                                log::error!("Failed to update ASR config: {:?}", e);
                                lcd::display_text(display, &format!("Failed to update ASR config:\n{:?}", e), 0).unwrap();
                            }
                        }
                        continue;
                    }
                    controller_evt => controller_evt,
                }
            }
        };

        if let (Some(driver), Some(asr_config)) = (driver.as_mut(), asr_config.as_ref()) {
            if matches!(
                event,
                bt_keyboard_mode::ControllerCommand::KeyboardPress(bt_keyboard_mode::KeysPin::MIC)
            ) {
                match driver.start_asr(
                    asr_config,
                    || lcd::display_text(display, "start recording", 0).unwrap(),
                    || key_pins.mic.is_high(),
                ) {
                    Ok(asr) => {
                        lcd::display_text(display, &format!("ASR:{asr}"), 0).unwrap();
                        controller.notify_asr(&asr);
                    }
                    Err(e) => {
                        log::error!("ASR error: {:?}", e);
                        lcd::display_text(display, &format!("ASR error: {:?}", e), 0).unwrap();
                    }
                }
                continue;
            }
        }

        let _ = handle_key_event(display, ble_device, keyboard, event, keymap);
    }
}

// Execute key action based on keymap configuration
fn execute_key_action(
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    action: &bt_keyboard_mode::KeyAction,
    is_press: bool,
) -> anyhow::Result<()> {
    use bt_keyboard_mode::KeyAction;

    match action {
        KeyAction::Combo { modifiers, key, .. } => {
            if is_press {
                // Apply modifiers and press key
                let mut modifier_mask = 0u8;

                for mod_name in modifiers {
                    match mod_name.as_str() {
                        "ctrl" => modifier_mask |= 0x01,
                        "shift" => modifier_mask |= 0x02,
                        "alt" | "option" => modifier_mask |= 0x04,
                        "meta" | "command" | "cmd" | "win" | "gui" => modifier_mask |= 0x08,
                        _ => {}
                    }
                }

                // Convert key name to HID code (may include modifier bit for modifier keys)
                let (key_code, key_modifier) = bt_keyboard_mode::key_name_to_hid_code(key)?;
                keyboard.press_raw(key_code, modifier_mask | key_modifier);
            } else {
                keyboard.release();
            }
        }
        KeyAction::Text { value, .. } => {
            if is_press {
                keyboard.write(value);
            }
        }
    }

    Ok(())
}

pub fn handle_key_event(
    display: &mut lcd::FrameBuffer,
    ble_device: &mut esp32_nimble::BLEDevice,
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    event: bt_keyboard_mode::ControllerCommand,
    keymap: &bt_keyboard_mode::KeymapConfig,
) -> anyhow::Result<()> {
    log::info!("Handling controller command: {:?}", event);
    use bt_keyboard_mode::KeysPin;
    match event {
        bt_keyboard_mode::ControllerCommand::Paste(p) => {
            if p == 0x01 {
                keyboard.ctrl_press(b'v');
                keyboard.release();
            } else if p == 0x02 {
                keyboard.gui_press(b'v');
                keyboard.release();
            }
        }
        bt_keyboard_mode::ControllerCommand::DisplayKeyboard(text) => {
            lcd::display_text(display, &text, 0)?;
        }
        bt_keyboard_mode::ControllerCommand::KeyboardPress(pin_index) => {
            if pin_index == KeysPin::ACCEPT {
                log::info!("Accept button pressed, starting advertising");
                let mut adv = ble_device.get_advertising().lock();
                if !adv.is_advertising() {
                    adv.start().unwrap();
                }
            }

            let key_name = bt_keyboard_mode::KeymapConfig::get_key_name(pin_index);
            if let Some(action) = keymap.keys.get(key_name) {
                log::info!("Executing custom keymap for {}: {:?}", key_name, action);
                let _ = execute_key_action(keyboard, action, true);
            } else {
                // Default behavior
                match pin_index {
                    KeysPin::MIC => keyboard.press_raw(0xE2, 0x01 | 0x04), // Ctrl + Option
                    KeysPin::CUSTOM => keyboard.write("/compact\n"),
                    KeysPin::ESC => keyboard.press_raw(0x29, 0),
                    KeysPin::NEXT => keyboard.press_raw(0x51, 0), // Down arrow
                    KeysPin::SWITCH => keyboard.shift_press(b'\t'),
                    KeysPin::BACKSPACE => keyboard.press_raw(0x2a, 0),
                    KeysPin::ACCEPT => {
                        keyboard.press(b'\n');
                    }
                    KeysPin::ROTATE_BUTTON => keyboard.write("/"),
                    _ => {}
                }
            }
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(pin_index) => {
            let key_name = bt_keyboard_mode::KeymapConfig::get_key_name(pin_index);
            if let Some(action) = keymap.keys.get(key_name) {
                log::info!("Releasing custom keymap for {}: {:?}", key_name, action);
                let _ = execute_key_action(keyboard, action, false);
            } else {
                // Default behavior
                match pin_index {
                    KeysPin::MIC
                    | KeysPin::CUSTOM
                    | KeysPin::NEXT
                    | KeysPin::SWITCH
                    | KeysPin::BACKSPACE
                    | KeysPin::ACCEPT
                    | KeysPin::ESC
                    | KeysPin::ROTATE_BUTTON => {
                        keyboard.release();
                    }
                    _ => {}
                }
            }
        }
        bt_keyboard_mode::ControllerCommand::RotateDown => {
            keyboard.mouse_move(0, 0, -1, 0); // Wheel down
        }
        bt_keyboard_mode::ControllerCommand::RotateUp => {
            keyboard.mouse_move(0, 0, 1, 0); // Wheel up
        }
        bt_keyboard_mode::ControllerCommand::KeymapConfig(_) => {
            // KeymapConfig is handled separately in keyboard_mode_main
        }
        bt_keyboard_mode::ControllerCommand::AsrConfig(_) => {
            // AsrConfig is handled separately in keyboard_mode_main
        }
    }

    Ok(())
}

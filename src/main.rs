use std::sync::{Arc, Mutex};

use embedded_graphics::prelude::{Dimensions, WebColors};
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
mod mqtt;
mod new_jpg;
mod protocol;
mod ui;
mod util;
mod wifi;

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
    "time.windows.com",
    "time.google.com",
    "ntp.aliyun.com",
    "time.cloudflare.com",
];

pub fn sync_time(display_target: &mut lcd::FrameBuffer) -> anyhow::Result<()> {
    use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncMode, SyncStatus};

    log_heap();
    log::info!(
        "SNTP sync time (parallel, {} servers)",
        DEFAULT_SNTP_SERVERS.len()
    );

    // 一次配齐所有 server:ESP-IDF SNTP 模块并发查询,谁先回就用谁(不再串行每个等 15s)。
    let conf = SntpConf {
        servers: DEFAULT_SNTP_SERVERS,
        operating_mode: OperatingMode::Poll,
        sync_mode: SyncMode::Immediate,
    };
    let ntp_client = EspSntp::new(&conf)?;

    for i in 0..15 {
        let p = ".".repeat(i % 4);
        let _ =
            ui::render_keyboard_view(display_target, false, false, &format!("Syncing time{}", p));
        let status = ntp_client.get_sync_status();
        log::info!("sntp sync status {:?}", status);
        log_heap();
        if status == SyncStatus::Completed {
            let _ =
                ui::render_keyboard_view(display_target, false, false, "Syncing time Completed");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    Err(anyhow::anyhow!("Failed to sync time via SNTP"))
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
    let _ = ui::render_keyboard_view(
        &mut target,
        false,
        false,
        "VibeKeys Starting...\n Read setting",
    );

    // MIC(远程模式下由 app::run 直接持有,用于本地 ASR,故声明为 mut)
    let mut btn0 = new_btn(
        peripherals.pins.gpio0.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // NEXT
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

    // Custom
    let btn2 = new_btn(
        peripherals.pins.gpio2.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Backspace
    let mut btn5 = new_btn(
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
    let mut pin16 = new_btn(
        peripherals.pins.gpio16.into(),
        esp_idf_svc::hal::gpio::Pull::Up,
        esp_idf_svc::hal::gpio::InterruptType::AnyEdge,
    )?;

    // Rotate B
    let mut pin17 = new_btn(
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

    let mut setting = bt_wifi_mode::Setting::load_from_nvs(&nvs)?;
    // Load keymap config before moving nvs
    let mut keymap = bt_keyboard_mode::KeymapConfig::load_from_nvs(&nvs)?;
    log::info!("Loaded keymap config with {} keys", keymap.keys.len());
    let asr_config = audio::AsrConfig::load_from_nvs(&nvs);

    let mut wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None)?;
    let mac = wifi.sta_netif().get_mac().unwrap();
    let client_id = format!(
        "vibekeys-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let scan_list = match wifi::scan(&mut wifi, sysloop.clone()) {
        Ok(list) => list,
        Err(e) => {
            log::error!("Failed to scan WiFi networks: {:?}", e);
            let _ = ui::render_keyboard_view(
                &mut target,
                false,
                false,
                &format!("Failed to scan WiFi networks:\n{:?}", e),
            );
            vec![]
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    if let Err(e) = runtime {
        log::error!("Failed to create Tokio runtime: {:?}", e);
        let _ = ui::render_keyboard_view(
            &mut target,
            false,
            false,
            &format!("Failed to create Tokio runtime:\n{:?}", e),
        );
        std::thread::sleep(std::time::Duration::from_secs(5));
        esp_idf_svc::hal::reset::restart();
    }

    let runtime = runtime.unwrap();

    let mode = loop {
        let choice = runtime.block_on(ui::boot_menu(&mut target, &mut btn7, &mut btn3, &mut btn4));
        match choice {
            ui::BootChoice::Keyboard => break 3,
            ui::BootChoice::Remote => break 1,
            ui::BootChoice::Setting => {
                match runtime.block_on(ui::setting_page(
                    &mut target,
                    &scan_list,
                    &mut btn7,
                    &mut btn3,
                    &mut btn4,
                    &mut pin16,
                    &mut pin17,
                    &mut btn5,
                    &mut setting,
                    &mut nvs,
                )) {
                    ui::SettingOutcome::Back => continue,
                    ui::SettingOutcome::Ota => {
                        let mut popup = ui::popup_centered(target.bounding_box());
                        let _ = popup.show_transient(&mut target, "Entering OTA...");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        goto_next_firmware()?;
                    }
                    ui::SettingOutcome::ClearConfig => {
                        bt_wifi_mode::Setting::clear_nvs(&mut nvs)?;
                        bt_keyboard_mode::KeymapConfig::clear_nvs(&mut nvs)?;
                        let mut popup = ui::popup_centered(target.bounding_box());
                        let _ = popup.show_transient(&mut target, "Clear all config");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        // 清空后重启:让(已清空的)配置重新加载,避免继续用内存里的旧值。
                        esp_idf_svc::hal::reset::restart();
                    }
                }
            }
        }
    };

    {
        let mut ota = esp_idf_svc::ota::EspOta::new()?;
        ota.mark_running_slot_valid()?;
    }

    if mode == 1 && setting.need_init() {
        let _ = ui::render_keyboard_view(
            &mut target,
            false,
            false,
            "Remote Control mode requires network/server config",
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    if mode == 3 || setting.need_init() {
        let _ = ui::render_keyboard_view(&mut target, false, false, "Starting in keyboard mode...");
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

        // 用 boot 阶段的扫描结果与已配置 wifi_list 匹配,挑当前在范围内的网络连接。
        let r = match bt_wifi_mode::pick_cred(&scan_list, &setting.wifi_list) {
            Some(c) => wifi::connect(&mut wifi, &c.ssid, &c.pass, sysloop.clone()),
            None => {
                log::error!(
                    "No known WiFi network in range (scan_list has {})",
                    scan_list.len()
                );
                anyhow::Result::<()>::Err(anyhow::anyhow!("no known network in range"))
            }
        };
        let wifi_on = r.is_ok();
        if r.is_err() {
            let e = r.err();
            log::error!("Failed to connect to WiFi: {:?}", e);
            let _ = ui::render_keyboard_view(
                &mut target,
                false,
                false,
                &format!(" WiFi connection failed: {:?}\n", e),
            );
            std::thread::sleep(std::time::Duration::from_secs(3));
        } else {
            log::info!("WiFi connected successfully");
            log::info!("ASR config loaded from NVS: {:?}", asr_config);

            if let Some(ref asr_config) = asr_config {
                // 关闭「优先内置 ASR」时键盘模式不会用 Whisper(MIC 透传给主机),
                // 也就不需要为 HTTPS 证书校验同步时间 —— 跳过省一段启动耗时。
                if setting.prefer_builtin_asr && asr_config.requires_tls() {
                    let r = sync_time(&mut target);
                    if r.is_err() {
                        log::error!("Failed to sync time: {:?}", r.err());
                        let _ = ui::render_keyboard_view(
                            &mut target,
                            false,
                            false,
                            " Time sync failed\n",
                        );
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
        let _ = ui::render_keyboard_view(&mut target, false, false, "Keyboard Mode");

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
            wifi_on,
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
        // btn0 (MIC) 由 app::run 直接持有用于本地 ASR,不在此 spawn。

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

    let _ = ui::render_keyboard_view(&mut target, false, false, "Connecting the WiFi...");

    // 用 boot 阶段的扫描结果与已配置 wifi_list 匹配,挑当前在范围内的网络连接。
    let r = match bt_wifi_mode::pick_cred(&scan_list, &setting.wifi_list) {
        Some(c) => wifi::connect(&mut wifi, &c.ssid, &c.pass, sysloop.clone()),
        None => {
            log::error!(
                "No known WiFi network in range (scan_list has {})",
                scan_list.len()
            );
            anyhow::Result::<()>::Err(anyhow::anyhow!("no known network in range"))
        }
    };
    if r.is_err() {
        log::error!("Failed to connect to WiFi: {:?}", r.err());
        let _ = ui::render_keyboard_view(&mut target, false, false, " WiFi connection failed\n");
        std::thread::sleep(std::time::Duration::from_secs(60));
        esp_idf_svc::hal::reset::restart();
    }

    if setting.server_url.starts_with("mqtts")
        || asr_config.as_ref().map_or(false, |c| c.requires_tls())
    {
        let _ = ui::render_keyboard_view(&mut target, false, false, "Syncing time...");
        let r = sync_time(&mut target);
        if r.is_err() {
            log::error!("Failed to sync time: {:?}", r.err());
            let _ = ui::render_keyboard_view(&mut target, false, false, " Time sync failed\n");
            std::thread::sleep(std::time::Duration::from_secs(60));
            esp_idf_svc::hal::reset::restart();
        }
    }

    // 远程模式改用本地 ASR(MQTT 无语音通道):创建 audio::Driver 持有 I2S,
    // 不再把音频流发给服务器。
    let worker = audio::AudioWorker {
        in_i2s: peripherals.i2s0,
        in_ws: peripherals.pins.gpio41.into(),
        in_clk: peripherals.pins.gpio42.into(),
        din: peripherals.pins.gpio40.into(),
        in_mclk: None,
    };
    let driver = audio::Driver::new(worker)
        .map_err(|e| log::error!("Failed to create audio driver: {e:?}"))
        .ok();

    log::info!("start ASR worker thread");
    log_heap();

    // ASR 跑在独立 OS 线程上,栈 64KB(与主任务一致,够跑 Whisper HTTP+TLS 流式录音;
    // tokio::spawn_blocking 的池线程栈太小会溢出)。Driver 由该线程独占,app_fut 通过
    // channel 发请求/收结果,避免长阻塞冻死 async runtime 上的 MQTT keepalive。
    // app_fut 结束 → asr_tx drop → channel 关闭 → worker 的 recv() 返回 Err → 线程退出。
    let (asr_tx, asr_rx) = std::sync::mpsc::channel::<audio::AsrRequest>();
    if let Err(e) = std::thread::Builder::new()
        .name("asr-worker".to_string())
        .stack_size(1024 * 16)
        .spawn(move || {
            let mut driver = driver;
            while let Ok(req) = asr_rx.recv() {
                let r = match driver.as_mut() {
                    Some(d) => d.start_asr(
                        &req.config,
                        || {},
                        || req.cancel.load(std::sync::atomic::Ordering::Relaxed),
                    ),
                    None => Err(anyhow::anyhow!("audio driver unavailable")),
                };
                let _ = req.respond.send(r);
            }
            log::info!("ASR worker thread exited");
        })
    {
        log::error!("Failed to spawn ASR worker thread: {e:?}");
    }

    let _ = ui::render_keyboard_view(&mut target, false, false, "Connecting the Server...");

    let mut ui = lcd::UI::new_with_target(target);

    let app_fut = app::run(
        setting.server_url,
        &client_id,
        &mut ui,
        rx,
        &keymap,
        asr_tx,
        asr_config.as_ref(),
        app::key_task::MicMode::from(setting.mic_model),
        &mut btn0,
    );
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
    let lock = setting_arc.lock().unwrap();
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
        lock.0
            .wifi_list
            .first()
            .map(|c| c.ssid.as_str())
            .unwrap_or(""),
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
    wifi_on: bool,
) -> ! {
    let _ = ui::render_keyboard_view(
        display,
        true,
        ble_device.get_server().connected_count() > 0,
        "Keyboard",
    );
    let mut popup = ui::popup_centered(display.bounding_box());
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
                        let _ = ui::render_keyboard_view(display, false, false, "keymap updated!");
                        continue;
                    }
                    controller_evt => controller_evt,
                }
            }
        };

        // 每轮事件先关闭上一轮的弹窗(增量 restore),再处理新事件
        let _ = popup.hide(display);

        // 内置 ASR(Whisper)只在本设置开启、且驱动与配置都在时才接管 MIC;
        // 否则 MIC 按键透传给主机(默认映射成 Ctrl+Option,触发主机自带听写)。
        let prefer_builtin_asr = setting_arc.lock().unwrap().0.prefer_builtin_asr;
        if let (Some(driver), Some(asr_config)) = (driver.as_mut(), asr_config.as_ref()) {
            if prefer_builtin_asr
                && matches!(
                    event,
                    bt_keyboard_mode::ControllerCommand::KeyboardPress(
                        bt_keyboard_mode::KeysPin::MIC
                    )
                )
            {
                // 麦克风模式取自 setting_arc(每次触发都读最新值,setup 改了即时生效)。
                let mic_mode =
                    app::key_task::MicMode::from(setting_arc.lock().unwrap().0.mic_model);
                match mic_mode {
                    app::key_task::MicMode::PushToTalk => {
                        // 按住说话:松手(is_high)停止 —— 现状不变。
                        match driver.start_asr(
                            asr_config,
                            || {
                                let _ = popup.show(display, "recording...");
                            },
                            || key_pins.mic.is_high(),
                        ) {
                            Ok(asr) => {
                                let _ = popup.show(display, &asr);
                                controller.notify_asr(&asr);
                            }
                            Err(e) => {
                                log::error!("ASR error: {:?}", e);
                                let _ = popup.show(display, "ASR error");
                            }
                        }
                    }
                    app::key_task::MicMode::Toggle => {
                        // 按一下开始、再按一下停止。start_asr 同步阻塞本事件循环,第二次
                        // 按下无法作为 KeyboardPress 事件到达,只能在 is_stop 里轮询引脚电平
                        // 做状态机:state 0 = 等首按松开;state 1 = 等第二次按下 → 返回 true 停止。
                        // is_stop 是 FnMut,可直接捕获可变 state,不必用原子。
                        let mut state: u8 = 0;
                        match driver.start_asr(
                            asr_config,
                            || {
                                let _ = popup.show(display, "recording...");
                            },
                            || {
                                if key_pins.mic.is_low() {
                                    // 已松开过(state 1)之后的按下即第二次 → 停止
                                    state == 1
                                } else {
                                    if state == 0 {
                                        state = 1;
                                    }
                                    false
                                }
                            },
                        ) {
                            Ok(asr) => {
                                let _ = popup.show(display, &asr);
                                controller.notify_asr(&asr);
                            }
                            Err(e) => {
                                log::error!("ASR error: {:?}", e);
                                let _ = popup.show(display, "ASR error");
                            }
                        }
                    }
                }
                continue;
            }
        }

        match &event {
            bt_keyboard_mode::ControllerCommand::KeyboardPress(pin) => {
                log::info!("Physical key pressed: {:?}", pin);
            }
            bt_keyboard_mode::ControllerCommand::KeyboardRelease(pin) => {
                log::info!("Physical key released: {:?}", pin);
            }
            _ => {}
        }

        let _ = handle_key_event(
            display, ble_device, keyboard, event, keymap, key_pins, wifi_on,
        )
        .await;
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

pub async fn handle_key_event(
    display: &mut lcd::FrameBuffer,
    ble_device: &mut esp32_nimble::BLEDevice,
    keyboard: &mut bt_keyboard_mode::KeyboardAndMouse,
    event: bt_keyboard_mode::ControllerCommand,
    keymap: &bt_keyboard_mode::KeymapConfig,
    key_pins: &mut bt_keyboard_mode::KeysPin,
    wifi_on: bool,
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
            let _ = ui::render_keyboard_view(display, wifi_on, true, &text);
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

            key_pins.wait_for_high(pin_index).await?;
            keyboard.release();
        }
        bt_keyboard_mode::ControllerCommand::KeyboardRelease(pin_index) => {
            let key_name = bt_keyboard_mode::KeymapConfig::get_key_name(pin_index);
            if let Some(action) = keymap.keys.get(key_name) {
                log::info!("Releasing custom keymap for {}: {:?}", key_name, action);
                let _ = execute_key_action(keyboard, action, false);
            } else {
                keyboard.release();
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
    }

    Ok(())
}

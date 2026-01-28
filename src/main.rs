use esp_idf_svc::hal::i2c::I2C0;

mod bt_keyboard;
mod i2c;
mod wifi;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;
    let partition = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;

    let keys_map = esp_idf_svc::nvs::EspNvs::new(partition.clone(), "keys_map", true)?;

    let mut ota = esp_idf_svc::ota::EspOta::new()?;

    esp32_nimble::BLEDevice::set_device_name("VibeKeys-MAX")?;
    let ble_device = esp32_nimble::BLEDevice::take();

    let mut keyboard_and_mouse = bt_keyboard::KeyboardAndMouse::new(ble_device, 100)?;
    let (controller_service, tx) = bt_keyboard::new_controller_service(ble_device)?;

    let server = ble_device.get_server();

    // server.advertise_on_disconnect(true);
    server.on_authentication_complete(move |a, b, c| {
        log::info!("Authentication complete: conn_result={:?}", c);
    });
    server.on_confirm_pin(move |a| {
        log::info!("Confirm PIN: {}", a);
        true
    });
    server.on_connect(move |a, b| {
        log::info!("Client connected: desc={:?}", b);
    });

    server.on_disconnect(move |a, b| {
        log::info!("Client disconnected: desc={:?}", b);
    });

    server.start()?;

    std::thread::sleep(std::time::Duration::from_millis(500));

    bt_keyboard::start_ble_advertising(ble_device, keyboard_and_mouse.hid_service_id())?;

    let r = start_lcd(
        peripherals.i2c0,
        peripherals.pins.gpio48,
        peripherals.pins.gpio45,
    );

    if let Err(e) = r {
        log::error!("Failed to start LCD: {:?}", e);
        controller_service.notify("LCD Init Failed");
        return Err(e);
    }

    let mut framebuffer = i2c::new_lcd_text_buffer();
    framebuffer.clear(i2c::Color::Off)?;
    i2c::lcd_display_bitmap(&framebuffer)?;
    i2c::lcd_display_text(&mut framebuffer, "VibeKeys Ready")?;

    let keys_pin = bt_keyboard::KeysPin(
        peripherals.pins.gpio0,
        peripherals.pins.gpio1,
        peripherals.pins.gpio2,
        peripherals.pins.gpio3,
        peripherals.pins.gpio4,
        peripherals.pins.gpio5,
        peripherals.pins.gpio6,
        peripherals.pins.gpio7,
        peripherals.pins.gpio18,
    );

    std::thread::Builder::new()
        .name("key_listener".to_string())
        .stack_size(1024 * 8)
        .spawn(move || {
            if let Err(e) = bt_keyboard::start_key_listen(tx, keys_pin) {
                log::error!("Key listening task failed: {:?}", e);
            }
        })?;

    if let Err(e) = ota.mark_running_slot_valid() {
        log::error!("Failed to mark running slot valid: {:?}", e);
        controller_service.notify("OTA Mark Valid Failed");
        return Err(anyhow::anyhow!("OTA mark running slot valid failed"));
    }

    while let Ok(event) = controller_service.rx.recv() {
        match event {
            bt_keyboard::ControllerCommand::GoToOta => {
                ota.mark_running_slot_invalid_and_reboot();
            }
            bt_keyboard::ControllerCommand::DisplayKeyboard(text) => {
                println!("Display on LCD: {}", text);
                i2c::lcd_display_text(&mut framebuffer, &text)?;
            }
            bt_keyboard::ControllerCommand::KeyboardPress(keycode) => {
                {
                    let mut adv = ble_device.get_advertising().lock();
                    log::info!("Checking advertising state... {}", adv.is_advertising());
                    if !adv.is_advertising() {
                        adv.start()?;
                    }
                }

                // keyboard_and_mouse.press_key(keycode)?;
                println!("Key Pressed: {}", keycode);
                handle_key_event(&mut keyboard_and_mouse, keycode, true)?;
            }
            bt_keyboard::ControllerCommand::KeyboardRelease(keycode) => {
                // keyboard_and_mouse.release_key(keycode)?;
                println!("Key Released: {}", keycode);
                handle_key_event(&mut keyboard_and_mouse, keycode, false)?;
            }
        }
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

pub struct KeysMap {
    pub key0: String,
    pub key1: String,
    pub key2: String,
    pub key3: String,
    pub key4: String,
    pub key5: String,
    pub key6: String,
    pub key7: String,
    pub key18: String,
}

impl KeysMap {
    pub fn load_from_nvs(keys_map: &esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<Self> {
        let mut s_buf = vec![0u8; 128];
        fn read_str_from_nvs(
            nvs: &esp_idf_svc::nvs::EspDefaultNvs,
            key: &str,
            buf: &mut Vec<u8>,
        ) -> String {
            nvs.get_str(key, buf)
                .ok()
                .flatten()
                .unwrap_or_default()
                .to_string()
        }
        let key0 = read_str_from_nvs(keys_map, "key0", &mut s_buf);
        let key1 = read_str_from_nvs(keys_map, "key1", &mut s_buf);
        let key2 = read_str_from_nvs(keys_map, "key2", &mut s_buf);
        let key3 = read_str_from_nvs(keys_map, "key3", &mut s_buf);
        let key4 = read_str_from_nvs(keys_map, "key4", &mut s_buf);
        let key5 = read_str_from_nvs(keys_map, "key5", &mut s_buf);
        let key6 = read_str_from_nvs(keys_map, "key6", &mut s_buf);
        let key7 = read_str_from_nvs(keys_map, "key7", &mut s_buf);
        let key18 = read_str_from_nvs(keys_map, "key18", &mut s_buf);

        Ok(KeysMap {
            key0,
            key1,
            key2,
            key3,
            key4,
            key5,
            key6,
            key7,
            key18,
        })
    }

    pub fn apply_to_keyboard(
        &self,
        keyboard: &mut bt_keyboard::KeyboardAndMouse,
        code: u8,
        pressed: bool,
    ) -> anyhow::Result<()> {
        if pressed {
            match code {
                0 => {
                    keyboard.write("/compact");
                    &self.key0
                }
                1 => &self.key1,
                2 => {
                    keyboard.shift_press(b'\t');
                    &self.key2
                }
                3 => {
                    keyboard.press(0); // ESC
                    &self.key3
                }
                4 => {
                    keyboard.write("retry");
                    &self.key4
                }
                5 => &self.key5,
                6 => &self.key6,
                7 => {
                    keyboard.press(b'\n'); // Enter
                    &self.key7
                }
                18 => &self.key18,
                _ => return Ok(()),
            };
        } else {
            keyboard.release();
        }

        Ok(())
    }
}

fn start_lcd(
    i2c: I2C0,
    sda: esp_idf_svc::hal::gpio::Gpio48,
    scl: esp_idf_svc::hal::gpio::Gpio45,
) -> anyhow::Result<()> {
    i2c::i2c_init(i2c, sda, scl)?;

    i2c::init_i2c_lcd()?;

    Ok(())
}

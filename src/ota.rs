use std::sync::Arc;

use esp32_nimble::{utilities::BleUuid, uuid128, BLEAdvertisementData, NimbleProperties};

mod wifi;

const SERVICE_ID: BleUuid = uuid128!("2eb410d4-b1f2-4634-b34f-e183cd4974f0");
const FIRMWARE_UPDATE_ID: BleUuid = uuid128!("c0ffee00-1234-5678-9abc-def012345678");
const STATE_CHAR_ID: BleUuid = uuid128!("bb50a00b-499c-4f47-b24f-b5dd08850121");

pub enum OTAEvent {
    FirmwareUpdate {
        ssid: String,
        password: String,
        url: String,
    },
    JustReboot,
}

pub struct OTARx {
    pub state_char: Arc<esp32_nimble::utilities::mutex::Mutex<esp32_nimble::BLECharacteristic>>,
    pub ota_event_rx: std::sync::mpsc::Receiver<OTAEvent>,
}

impl OTARx {
    pub fn notify_state(&self, state: &str) {
        self.state_char.lock().set_value(state.as_bytes()).notify();
    }
}

pub fn bt(device_prefix: &str, ble_device: &mut esp32_nimble::BLEDevice) -> anyhow::Result<OTARx> {
    let (ota_event_tx, ota_event_rx) = std::sync::mpsc::channel::<OTAEvent>();

    let bt_mac = ble_device.get_addr()?;

    let ble_advertising = ble_device.get_advertising();

    let server = ble_device.get_server();
    server.on_connect(|server, desc| {
        log::info!("Client connected: {:?}", desc);

        server
            .update_conn_params(desc.conn_handle(), 24, 48, 0, 60)
            .unwrap();

        if server.connected_count() < (esp_idf_svc::sys::CONFIG_BT_NIMBLE_MAX_CONNECTIONS as _) {
            log::info!("Multi-connect support: start advertising");
            ble_advertising.lock().start().unwrap();
        }
    });

    server.on_disconnect(|_desc, reason| {
        log::info!("Client disconnected ({:?})", reason);
    });

    let service = server.create_service(SERVICE_ID);

    let ota_event_tx_ = ota_event_tx.clone();
    let firmware_update_characteristic = service
        .lock()
        .create_characteristic(FIRMWARE_UPDATE_ID, NimbleProperties::WRITE);
    firmware_update_characteristic.lock().on_write(move |args| {
        log::info!("Wrote to firmware update characteristic");
        let ssid_and_password = args.recv_data().to_vec();

        if ssid_and_password.is_empty() {
            log::info!("Received empty firmware update data, rebooting to next firmware");
            let _ = ota_event_tx_.send(OTAEvent::JustReboot);
            return;
        }

        let ssid_and_password_str = String::from_utf8(ssid_and_password);

        if let Ok(ssid_and_password_str) = ssid_and_password_str {
            let parts: Vec<&str> = ssid_and_password_str.split('\n').collect();
            if parts.len() == 3 {
                let ssid = parts[0].to_string();
                let password = parts[1].to_string();
                let url = parts[2].to_string();

                let _ = ota_event_tx_.send(OTAEvent::FirmwareUpdate {
                    ssid,
                    password,
                    url,
                });
            } else {
                log::error!("Invalid firmware update data format");
                args.reject();
            }
        } else {
            log::error!("Failed to parse firmware update data as UTF-8");
            args.reject();
        }
    });

    let state_characteristic = service.lock().create_characteristic(
        STATE_CHAR_ID,
        NimbleProperties::NOTIFY | NimbleProperties::READ,
    );

    let addr = bt_mac.to_string();
    ble_advertising.lock().set_data(
        BLEAdvertisementData::new()
            .name(&format!("{}-{}", device_prefix, addr))
            .add_service_uuid(SERVICE_ID),
    )?;
    ble_advertising.lock().start()?;

    Ok(OTARx {
        state_char: state_characteristic,
        ota_event_rx,
    })
}

pub fn ota_main() -> anyhow::Result<()> {
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();

    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;

    let ota_device_name = std::option_env!("OTA_DEVICE_NAME").unwrap_or("OTADevice");
    let ble_device = esp32_nimble::BLEDevice::take();

    let mut ota_rx = bt(ota_device_name, ble_device)?;
    ota_rx.notify_state("Ready for OTA");

    let esp_wifi = esp_idf_svc::wifi::EspWifi::new(peripherals.modem, sysloop.clone(), None);
    if esp_wifi.is_err() {
        log::error!("Failed to create EspWifi: {:?}", esp_wifi.err());
        ota_rx.notify_state("Failed to create EspWifi");
        return Err(anyhow::anyhow!("Failed to create EspWifi"));
    }

    let mut esp_wifi = esp_wifi.unwrap();

    // ota_test();

    if let Err(e) = start_ota(&mut ota_rx, &mut esp_wifi, sysloop) {
        log::error!("OTA process failed: {:?}", e);
        ota_rx.notify_state(&format!("OTA process failed: {:?}", e));
        return Err(anyhow::anyhow!("OTA process failed: {:?}", e));
    }

    Ok(())
}

pub fn start_ota(
    rx: &mut OTARx,
    esp_wifi: &mut esp_idf_svc::wifi::EspWifi<'static>,
    sysloop: esp_idf_svc::eventloop::EspSystemEventLoop,
) -> anyhow::Result<()> {
    let mut ota = esp_idf_svc::ota::EspOta::new()?;
    ota.mark_running_slot_valid()?;
    loop {
        match rx.ota_event_rx.recv() {
            Ok(event) => match event {
                OTAEvent::JustReboot => {
                    log::info!("Received JustReboot event");
                    rx.notify_state("Rebooting to next firmware...");
                    goto_next_firmware()?;
                }
                OTAEvent::FirmwareUpdate {
                    ssid,
                    password,
                    url,
                } => {
                    log::info!(
                        "Received OTA firmware update request: ssid='{}', password='{}', url='{}'",
                        ssid,
                        password,
                        url
                    );

                    rx.notify_state("Starting OTA update...");
                    let r = crate::wifi::connect(esp_wifi, &ssid, &password, sysloop.clone());

                    if let Err(e) = r {
                        log::error!("Failed to connect to WiFi: {:?}", e);
                        rx.notify_state("Failed to connect to WiFi");
                        continue;
                    } else {
                        log::info!("Connected to WiFi successfully");
                        rx.notify_state("Connected to WiFi successfully");
                    }

                    if let Err(e) = get_framework_from_url(&url, rx, &mut ota).map_err(|e| {
                        log::error!("Failed to download firmware: {:?}", e);
                        anyhow::anyhow!("Failed to download firmware: {:?}", e)
                    }) {
                        log::error!("OTA update failed: {:?}", e);
                        rx.notify_state("OTA update failed");
                        continue;
                    } else {
                        log::info!("OTA update downloaded successfully");
                        rx.notify_state(
                            "OTA update downloaded successfully. Awaiting confirmation...",
                        );
                    }
                }
            },
            Err(_) => {
                return Err(anyhow::anyhow!("OTA event channel closed"));
            }
        }
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

pub fn get_framework_from_url(
    url: &str,
    rx: &mut OTARx,
    ota: &mut esp_idf_svc::ota::EspOta,
) -> anyhow::Result<()> {
    let mut update = ota.initiate_update().map_err(|e| {
        log::error!("Failed to initiate OTA update: {:?}", e);
        anyhow::anyhow!("Failed to initiate OTA update: {:?}", e)
    })?;

    let configuration = esp_idf_svc::http::client::Configuration::default();
    let conn = esp_idf_svc::http::client::EspHttpConnection::new(&configuration)?;
    let mut client = embedded_svc::http::client::Client::wrap(conn);
    let mut response = client.get(url)?.submit()?;

    rx.notify_state("Downloading firmware...");
    let mut nn = 0;
    let mut bytes_buffer: Vec<u8> = vec![0; 4096];
    log::info!("Receiving firmware data in chunks...");
    log_heap();

    let status = response.status();
    if status > 299 || status < 200 {
        log::error!("HTTP request failed with status: {}", status);
        return Err(anyhow::anyhow!(
            "HTTP request failed with status: {}",
            status
        ));
    }

    loop {
        let n = response.read(&mut bytes_buffer)?;
        nn += n;
        if n == 0 {
            break;
        }
        if nn % 4096 == 0 {
            rx.notify_state(&format!("Downloaded {} KB ...", nn / 1024));
        }
        update.write(&bytes_buffer[..n]).map_err(|e| {
            log::error!("Failed to write OTA chunk: {:?}", e);
            anyhow::anyhow!("Failed to write OTA chunk: {:?}", e)
        })?;
    }
    log::info!("All chunks received");

    rx.notify_state(&format!(
        "Finished downloading firmware, total size: {} bytes",
        nn
    ));

    update.complete().map_err(|e| {
        log::error!("Failed to complete OTA update: {:?}", e);
        anyhow::anyhow!("Failed to complete OTA update: {:?}", e)
    })?;

    rx.notify_state("OTA update applied successfully. Rebooting after 5 seconds...");
    std::thread::sleep(std::time::Duration::from_secs(5));

    esp_idf_svc::hal::reset::restart();
}

pub fn goto_next_firmware() -> anyhow::Result<()> {
    use esp_idf_svc::sys::{esp_ota_get_next_update_partition, esp_ota_set_boot_partition};

    unsafe {
        let partition = esp_ota_get_next_update_partition(std::ptr::null());
        esp_idf_svc::sys::esp!(esp_ota_set_boot_partition(partition))?;
    };

    esp_idf_svc::hal::reset::restart();
}

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    if let Err(e) = ota_main() {
        log::error!("OTA main failed: {:?}", e);
    }
}

use std::sync::{Arc, Mutex};

use esp32_nimble::{utilities::BleUuid, uuid128, BLEAdvertisementData, NimbleProperties};

use crate::lcd;

const SERVICE_ID: BleUuid = uuid128!("623fa3e2-631b-4f8f-a6e7-a7b09c03e7e0");
const SSID_ID: BleUuid = uuid128!("1fda4d6e-2f14-42b0-96fa-453bed238375");
const PASS_ID: BleUuid = uuid128!("a987ab18-a940-421a-a1d7-b94ee22bccbe");
const SERVER_URL_ID: BleUuid = uuid128!("cef520a9-bcb5-4fc6-87f7-82804eee2b20");
const MIC_MODEL_ID: BleUuid = uuid128!("72ae1823-ab95-4d78-af01-4ce8bb88e034");
const BACKGROUND_PNG_ID: BleUuid = uuid128!("d1f3b2c4-5e6f-4a7b-8c9d-0e1f2a3b4c5d");
const RESET_ID: BleUuid = uuid128!("f0e1d2c3-b4a5-6789-0abc-def123456789");

#[derive(Debug, Clone)]
pub struct Setting {
    pub ssid: String,
    pub pass: String,
    pub server_url: String,
    pub background_png: (Vec<u8>, bool), // (data, ended)
    pub mic_model: u8,
    state: u8,
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

        static DEFAULT_SERVER_URL: Option<&str> = std::option_env!("DEFAULT_SERVER_URL");
        log::info!("DEFAULT_SERVER_URL: {:?}", DEFAULT_SERVER_URL);

        let server_url = nvs
            .get_str("server_url", &mut str_buf)
            .map_err(|e| log::error!("Failed to get server_url: {:?}", e))
            .ok()
            .flatten()
            .or(DEFAULT_SERVER_URL)
            .unwrap_or_default()
            .to_string();

        let background_png = if nvs.contains("background_png")? {
            let background_png_size = nvs
                .blob_len("background_png")
                .map_err(|e| log::error!("Failed to get background_png size: {:?}", e))
                .ok()
                .flatten()
                .unwrap_or(1024 * 1024);

            log::info!("Background PNG size in NVS: {} bytes", background_png_size);

            let mut png_buf = vec![0; background_png_size];
            let png_buf_ = nvs
                .get_blob("background_png", &mut png_buf)?
                .unwrap_or(lcd::DEFAULT_BACKGROUND);

            if png_buf_.len() != background_png_size {
                log::warn!(
                    "Background PNG size mismatch: expected {}, got {}",
                    background_png_size,
                    png_buf_.len()
                );
                png_buf_.to_vec()
            } else {
                png_buf
            }
        } else {
            log::info!("No background PNG found in NVS, using default.");
            lcd::DEFAULT_BACKGROUND.to_vec()
        };

        let state = nvs.get_u8("state")?.unwrap_or(0);
        nvs.set_u8("state", 0)?;

        let mic_model = nvs.get_u8("mic_model")?.unwrap_or(0);

        Ok(Setting {
            ssid,
            pass,
            server_url,
            background_png: (background_png, false),
            mic_model,
            state,
        })
    }

    pub fn need_init(&self) -> bool {
        self.state == 1
            || self.ssid.is_empty()
            || self.pass.is_empty()
            || self.server_url.is_empty()
    }
}

pub enum BTevent {
    Reset,
}

pub fn bt(
    device_id: &str,
    setting: Arc<Mutex<(Setting, esp_idf_svc::nvs::EspDefaultNvs)>>,
    evt_tx: std::sync::mpsc::Sender<BTevent>,
) -> anyhow::Result<()> {
    let ble_device = esp32_nimble::BLEDevice::take();
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

    let setting1 = setting.clone();
    let setting2 = setting.clone();

    let ssid_characteristic = service
        .lock()
        .create_characteristic(SSID_ID, NimbleProperties::READ | NimbleProperties::WRITE);
    ssid_characteristic
        .lock()
        .on_read(move |c, _| {
            log::info!("Read from SSID characteristic");
            let setting = setting1.lock().unwrap();
            c.set_value(setting.0.ssid.as_bytes());
        })
        .on_write(move |args| {
            log::info!(
                "Wrote to SSID characteristic: {:?} -> {:?}",
                args.current_data(),
                args.recv_data()
            );
            if let Ok(new_ssid) = String::from_utf8(args.recv_data().to_vec()) {
                log::info!("New SSID: {}", new_ssid);
                let mut setting = setting2.lock().unwrap();
                if let Err(e) = setting.1.set_str("ssid", &new_ssid) {
                    log::error!("Failed to save SSID to NVS: {:?}", e);
                } else {
                    setting.0.ssid = new_ssid;
                }
            } else {
                log::error!("Failed to parse new SSID from bytes.");
            }
        });

    let setting1 = setting.clone();
    let setting2 = setting.clone();
    let pass_characteristic = service
        .lock()
        .create_characteristic(PASS_ID, NimbleProperties::READ | NimbleProperties::WRITE);
    pass_characteristic
        .lock()
        .on_read(move |c, _| {
            log::info!("Read from pass characteristic");
            let setting = setting1.lock().unwrap();
            c.set_value(setting.0.pass.as_bytes());
        })
        .on_write(move |args| {
            log::info!(
                "Wrote to pass characteristic: {:?} -> {:?}",
                args.current_data(),
                args.recv_data()
            );
            if let Ok(new_pass) = String::from_utf8(args.recv_data().to_vec()) {
                log::info!("New pass: {}", new_pass);
                let mut setting = setting2.lock().unwrap();
                if let Err(e) = setting.1.set_str("pass", &new_pass) {
                    log::error!("Failed to save pass to NVS: {:?}", e);
                } else {
                    setting.0.pass = new_pass;
                }
            } else {
                log::error!("Failed to parse new pass from bytes.");
            }
        });

    let setting_sever_url_r = setting.clone();
    let setting_ = setting.clone();
    let setting_gif = setting.clone();

    let server_url_characteristic = service.lock().create_characteristic(
        SERVER_URL_ID,
        NimbleProperties::READ | NimbleProperties::WRITE,
    );
    server_url_characteristic
        .lock()
        .on_read(move |c, _| {
            log::info!("Read from server URL characteristic");
            let setting = setting_sever_url_r.lock().unwrap();
            c.set_value(setting.0.server_url.as_bytes());
        })
        .on_write(move |args| {
            log::info!(
                "Wrote to server URL characteristic: {:?} -> {:?}",
                args.current_data(),
                args.recv_data()
            );
            if let Ok(new_server_url) = String::from_utf8(args.recv_data().to_vec()) {
                log::info!("New server URL: {}", new_server_url);
                let mut setting = setting_.lock().unwrap();
                if let Err(e) = setting.1.set_str("server_url", &new_server_url) {
                    log::error!("Failed to save server URL to NVS: {:?}", e);
                } else {
                    setting.0.server_url = new_server_url;
                }
            } else {
                log::error!("Failed to parse new server URL from bytes.");
            }
        });

    let background_png_characteristic = service
        .lock()
        .create_characteristic(BACKGROUND_PNG_ID, NimbleProperties::WRITE);
    background_png_characteristic.lock().on_write(move |args| {
        let gif_chunk = args.recv_data();

        if gif_chunk.len() <= 1024 * 1024 && gif_chunk.len() > 0 {
            log::info!("New background GIF received, size: {}", gif_chunk.len());
            let mut setting = setting_gif.lock().unwrap();
            setting.0.background_png.0.extend_from_slice(gif_chunk);
            if gif_chunk.len() < 512 {
                setting.0.background_png.1 = true; // Mark as valid
            }
            if setting.0.background_png.0.len() > 1024 * 1024 {
                log::warn!("Background GIF size exceeds 1024KB, resetting to default.");
                setting.0.background_png.0.clear();
                setting.0.background_png.1 = false;
                args.reject();
            }
        } else {
            log::error!("Failed to parse new background GIF from bytes.");
        }
    });

    let setting = setting.clone();
    let setting_ = setting.clone();

    let mic_model_characteristic = service.lock().create_characteristic(
        MIC_MODEL_ID,
        NimbleProperties::READ | NimbleProperties::WRITE,
    );
    mic_model_characteristic.lock().on_read(move |c, _| {
        log::info!("Read from mic model characteristic");
        let setting = setting.lock().unwrap();
        c.set_value(&[setting.0.mic_model]);
    });
    mic_model_characteristic.lock().on_write(move |args| {
        log::info!(
            "Wrote to mic model characteristic: {:?} -> {:?}",
            args.current_data(),
            args.recv_data()
        );
        if let Some(&new_mic_model) = args.recv_data().get(0) {
            log::info!("New mic model: {}", new_mic_model);
            let mut setting = setting_.lock().unwrap();
            if let Err(e) = setting.1.set_u8("mic_model", new_mic_model) {
                log::error!("Failed to save mic model to NVS: {:?}", e);
            } else {
                setting.0.mic_model = new_mic_model;
            }
        } else {
            log::error!("Failed to parse new mic model from bytes.");
        }
    });

    let reset_characteristic = service
        .lock()
        .create_characteristic(RESET_ID, NimbleProperties::WRITE);
    reset_characteristic.lock().on_write(move |args| {
        let reset_cmd = args.recv_data();
        if reset_cmd == b"RESET" {
            evt_tx.send(BTevent::Reset).unwrap();
        } else {
            log::warn!("Invalid reset command received via BLE.");
        }
    });

    ble_advertising.lock().set_data(
        BLEAdvertisementData::new()
            .name(&format!("VibeKeys-Max-{}", device_id))
            .add_service_uuid(SERVICE_ID),
    )?;
    ble_advertising.lock().start()?;
    Ok(())
}

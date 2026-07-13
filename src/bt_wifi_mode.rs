use std::sync::{Arc, Mutex};

use esp32_nimble::{utilities::BleUuid, uuid128, BLEService, NimbleProperties};
use serde::{Deserialize, Serialize};

use crate::audio::AsrConfig;
use crate::lcd;

pub const SERVICE_ID: BleUuid = uuid128!("623fa3e2-631b-4f8f-a6e7-a7b09c03e7e0");
/// 统一配置特征值:承载 ssid / pass / server_url(JSON {type,value} 复用)。
/// 沿用原 SERVER_URL 的 UUID,手机端只对接这一个特征值。
const CONFIG_ID: BleUuid = uuid128!("cef520a9-bcb5-4fc6-87f7-82804eee2b20");
const BACKGROUND_PNG_ID: BleUuid = uuid128!("d1f3b2c4-5e6f-4a7b-8c9d-0e1f2a3b4c5d");
const RESET_ID: BleUuid = uuid128!("f0e1d2c3-b4a5-6789-0abc-def123456789");

/// NVS key holding the whole `wifi_list` as one JSON value.
pub const WIFI_LIST_KEY: &str = "wifi_list";

/// NVS key for `prefer_builtin_asr`。NVS key 上限 15 字符,"prefer_builtin_asr"(18)
/// 会触发 ESP_ERR_NVS_KEY_TOO_LONG,故缩写;JSON 的 type 字段和 Rust 字段名保持长名不变。
const PREFER_BUILTIN_ASR_KEY: &str = "prefer_asr";

/// 统一配置特征值的写入载荷:部分配置对象,如
/// `{"server_url":"...","prefer_builtin_asr":false}`。只出现(非 None)的字段才更新,
/// 缺失字段保持原状 —— 一次写可携带任意多项,免得改多项发多次。
/// 读取时返回 `ConfigSnapshot` 整份快照。
#[derive(Debug, Deserialize)]
struct ConfigSaveSnapshot {
    wifi_list: Option<Vec<WifiCred>>,
    server_url: Option<String>,
    asr_config: Option<serde_json::Value>,
    mic_model: Option<u8>,
    prefer_builtin_asr: Option<bool>,
}

/// 统一配置特征值的读取快照:整份 wifi_list + server_url + asr_config + mic_model + prefer_builtin_asr。
#[derive(Serialize)]
struct ConfigSnapshot<'a> {
    wifi_list: &'a [WifiCred],
    server_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    asr_config: Option<serde_json::Value>,
    mic_model: u8,
    prefer_builtin_asr: bool,
}
/// 单条 WiFi 凭据。顺序即连接优先级。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WifiCred {
    pub ssid: String,
    pub pass: String,
}

/// 最多保存多少个 WiFi 配置:把 JSON 体量压在 NVS 单值 ~4KB 限额内。
pub const MAX_WIFI_CREDS: usize = 8;

/// 在已配置凭据里找出第一个出现在扫描结果中的(顺序即优先级)。
/// 连接时用:不重新扫描,复用 boot 阶段的 `scan_list`。
pub fn pick_cred<'a>(scan_list: &[String], creds: &'a [WifiCred]) -> Option<&'a WifiCred> {
    creds
        .iter()
        .find(|c| scan_list.iter().any(|s| s == &c.ssid))
}

#[derive(Debug, Clone)]
pub struct Setting {
    /// 多个已配置 WiFi;连接时与扫描结果匹配,顺序即优先级。
    pub wifi_list: Vec<WifiCred>,
    pub server_url: String,
    pub background_png: (Vec<u8>, bool), // (data, ended)
    pub mic_model: u8,
    /// 键盘模式下是否优先用内置 ASR(Whisper);false 时 MIC 透传给主机(触发主机自带听写)。
    pub prefer_builtin_asr: bool,
    state: u8,
}

impl Setting {
    /// 把整个 wifi_list 作为一个 JSON 字符串写入 NVS。
    pub fn save_wifi_list(
        nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
        list: &[WifiCred],
    ) -> anyhow::Result<()> {
        let json = serde_json::to_string(list)?;
        nvs.set_str(WIFI_LIST_KEY, &json)?;
        Ok(())
    }

    pub fn clear_nvs(nvs: &mut esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<()> {
        nvs.remove(WIFI_LIST_KEY)?;
        nvs.remove("server_url")?;
        nvs.remove("background_png")?;
        nvs.remove("mic_model")?;
        nvs.remove(PREFER_BUILTIN_ASR_KEY)?;
        nvs.remove("state")?;
        Ok(())
    }

    pub fn load_from_nvs(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<Self> {
        let mut str_buf = [0; 128];

        // wifi_list 以单个 JSON 值存放。旧的 ssid/pass NVS 数据直接丢弃(不迁移)。
        let mut json_buf = [0u8; 4096];
        let wifi_list = nvs
            .get_str(WIFI_LIST_KEY, &mut json_buf)
            .map_err(|e| log::error!("Failed to get wifi_list: {:?}", e))
            .ok()
            .flatten()
            .and_then(|s| match serde_json::from_str::<Vec<WifiCred>>(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    log::error!("Failed to parse wifi_list JSON: {:?}", e);
                    None
                }
            })
            .unwrap_or_default();
        log::info!("Loaded {} wifi creds from NVS", wifi_list.len());

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

        let background_png_size = nvs
            .blob_len("background_png")
            .map_err(|e| log::error!("Failed to get background_png size: {:?}", e))
            .ok()
            .flatten()
            .unwrap_or(0);

        let background_png = if background_png_size != 0 {
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

        let mic_model = nvs.get_u8("mic_model")?.unwrap_or(1);
        let prefer_builtin_asr = nvs.get_u8(PREFER_BUILTIN_ASR_KEY)?.unwrap_or(1) != 0;

        Ok(Setting {
            wifi_list,
            server_url,
            background_png: (background_png, false),
            mic_model,
            prefer_builtin_asr,
            state,
        })
    }

    pub fn need_init(&self) -> bool {
        self.state == 1 || self.wifi_list.is_empty() || self.server_url.is_empty()
    }
}

pub enum BTevent {
    Reset,
}

pub fn new_setting_service(
    service: &mut BLEService,
    setting: Arc<Mutex<(Setting, esp_idf_svc::nvs::EspDefaultNvs)>>,
    evt_tx: Option<tokio::sync::mpsc::Sender<BTevent>>,
) -> anyhow::Result<()> {
    let config_r = setting.clone();
    let config_w = setting.clone();

    let config_characteristic =
        service.create_characteristic(CONFIG_ID, NimbleProperties::READ | NimbleProperties::WRITE);
    config_characteristic
        .lock()
        .on_read(move |c, _| {
            // 读一次返回整份快照(wifi_list + server_url + asr_config + mic_model),免去多次读。
            let setting = config_r.lock().unwrap();
            let asr_config = AsrConfig::load_from_nvs(&setting.1);
            log::info!(
                "Config read: asr_config = {}",
                asr_config.as_ref().map(|_| "present").unwrap_or("absent")
            );
            let asr_config = asr_config.and_then(|cfg| serde_json::to_value(&cfg).ok());
            let snap = ConfigSnapshot {
                wifi_list: &setting.0.wifi_list,
                server_url: setting.0.server_url.as_str(),
                asr_config,
                mic_model: setting.0.mic_model,
                prefer_builtin_asr: setting.0.prefer_builtin_asr,
            };
            match serde_json::to_string(&snap) {
                Ok(json) => {
                    c.set_value(json.as_bytes());
                }
                Err(e) => log::error!("Failed to serialize config snapshot: {:?}", e),
            }
        })
        .on_write(move |args| {
            // 写入载荷:部分配置对象,如 {"server_url":"...","prefer_builtin_asr":false}。
            // 只出现(非 None)的字段才更新,缺失字段保持原状 —— 一次写可携带任意多项。
            log::info!("Config write: {:?}", args.recv_data());
            let payload = match std::str::from_utf8(args.recv_data()) {
                Ok(s) => s,
                Err(_) => {
                    log::error!("Config write: payload not UTF-8");
                    return;
                }
            };
            log::info!("Config write payload: {}", payload);
            let save = match serde_json::from_str::<ConfigSaveSnapshot>(payload) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Config write: invalid JSON ({}): {}", e, payload);
                    return;
                }
            };
            log::info!("Config write: {:?}", save);
            let mut setting = config_w.lock().unwrap();

            if let Some(mut list) = save.wifi_list {
                if list.len() > MAX_WIFI_CREDS {
                    log::warn!(
                        "wifi_list has {} entries, truncating to {}",
                        list.len(),
                        MAX_WIFI_CREDS
                    );
                    list.truncate(MAX_WIFI_CREDS);
                }
                setting.0.wifi_list = list;
                // 经 MutexGuard 的 Deref,先 clone 再写 NVS,避免同时借 guard。
                let l = setting.0.wifi_list.clone();
                if let Err(e) = Setting::save_wifi_list(&mut setting.1, &l) {
                    log::error!("Failed to save wifi_list: {:?}", e);
                }
            }

            if let Some(url) = save.server_url {
                setting.0.server_url = url.clone();
                if let Err(e) = setting.1.set_str("server_url", &url) {
                    log::error!("Failed to save server_url: {:?}", e);
                }
            }

            if let Some(asr) = save.asr_config {
                // 合并写(默认值 < 现有 NVS < 本次传入):只覆盖传入里出现的 key,
                // 缺失的 key 保持原状 —— 不完整的 JSON 也能增量更新。
                // asr_config 走独立 NVS 键,重启后由 main.rs 重新加载。
                let mut base = AsrConfig::load_from_nvs(&setting.1)
                    .and_then(|c| serde_json::to_value(&c).ok())
                    .unwrap_or_else(|| {
                        serde_json::json!({"platform":"whisper","uri":"","api_key":"","model":""})
                    });
                if let (Some(base_obj), Some(in_obj)) = (base.as_object_mut(), asr.as_object()) {
                    for (k, v) in in_obj {
                        base_obj.insert(k.clone(), v.clone());
                    }
                }
                match serde_json::from_value::<AsrConfig>(base) {
                    Ok(cfg) => {
                        if let Err(e) = cfg.save_to_nvs(&mut setting.1) {
                            log::error!("Failed to save asr_config: {:?}", e);
                        }
                    }
                    Err(e) => log::error!("asr_config value invalid after merge: {:?}", e),
                }
            }

            if let Some(m) = save.mic_model {
                setting.0.mic_model = m;
                if let Err(e) = setting.1.set_u8("mic_model", m) {
                    log::error!("Failed to save mic_model: {:?}", e);
                }
            }

            if let Some(b) = save.prefer_builtin_asr {
                setting.0.prefer_builtin_asr = b;
                if let Err(e) = setting.1.set_u8(PREFER_BUILTIN_ASR_KEY, b as u8) {
                    log::error!("Failed to save prefer_builtin_asr: {:?}", e);
                }
            }
        });

    let setting_gif = setting.clone();

    let background_png_characteristic =
        service.create_characteristic(BACKGROUND_PNG_ID, NimbleProperties::WRITE);
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

    let evt_tx_reset = evt_tx.clone();
    let reset_characteristic = service.create_characteristic(RESET_ID, NimbleProperties::WRITE);
    reset_characteristic.lock().on_write(move |args| {
        let reset_cmd = args.recv_data();
        if reset_cmd == b"RESET" {
            if let Some(tx) = &evt_tx_reset {
                tx.blocking_send(BTevent::Reset).unwrap();
            } else {
                log::info!("Reset command received, but no event handler configured");
            }
        } else {
            log::warn!("Invalid reset command received via BLE.");
        }
    });

    Ok(())
}

use std::sync::{Arc, Mutex, OnceLock};

use esp32_nimble::{utilities::BleUuid, uuid128, BLECharacteristic, BLEService, NimbleProperties};
use serde::{Deserialize, Serialize};

use crate::audio::AsrConfig;

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
    /// Tailscale auth key(tskey-auth-...),remote 模式 broker 在 tailnet 内时用。
    /// 只写不读回快照;空字符串忽略。
    tailscale_key: Option<String>,
}

/// 统一配置特征值的读取快照:整份 wifi_list + server_url + asr_config + mic_model +
/// prefer_builtin_asr。keymap 不在这里 —— 走 KEYMAP_CONFIG_ID(READ|WRITE)单独读写,
/// 值为键名 → 绑定对象的 JSON(与写入格式一致)。
#[derive(Serialize)]
struct ConfigSnapshot<'a> {
    wifi_list: &'a [WifiCred],
    server_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    asr_config: Option<serde_json::Value>,
    mic_model: u8,
    prefer_builtin_asr: bool,
}

/// CONFIG 特征值的引用:refresh_config_cache 在主循环任务里刷新缓存时用。
/// 为什么需要主动刷新:esp32-nimble 对长度 > MTU-3 的特征值,普通 Read Request
/// **不触发 on_read**,直接把上次 set_value 缓存的旧值切给客户端(crate
/// ble_characteristic.rs 的 om_pkthdr_len 条件)。
static CONFIG_CHAR: OnceLock<Arc<esp32_nimble::utilities::mutex::Mutex<BLECharacteristic>>> =
    OnceLock::new();

/// 重建快照并写进 CONFIG 特征值缓存。由主循环在收到 BTevent::RefreshConfig 时
/// 调用 —— 那时已不在特征分发栈内,lock 这把锁是安全的(on_write 回调里做
/// 同样的事会自死锁,见该变体注释)。序列化失败打日志后跳过。
pub fn refresh_config_cache(setting: &Setting, nvs: &esp_idf_svc::nvs::EspDefaultNvs) {
    let Some(ch) = CONFIG_CHAR.get() else {
        return;
    };
    match config_snapshot_json(setting, nvs) {
        Some(json) => {
            ch.lock().set_value(json.as_bytes());
        }
        None => log::error!("Config cache refresh: failed to serialize snapshot"),
    }
}

/// 构建整份配置快照 JSON。on_read 与 refresh_config_cache 共用,保证两条路径
/// 吐出完全一致的内容。序列化失败返回 None(极少见:字段里出现不可序列化值)。
fn config_snapshot_json(
    setting: &Setting,
    nvs: &esp_idf_svc::nvs::EspDefaultNvs,
) -> Option<String> {
    let asr_config = AsrConfig::load_from_nvs(nvs);
    log::info!(
        "Config snapshot: asr_config = {}",
        asr_config.as_ref().map(|_| "present").unwrap_or("absent")
    );
    let asr_config = asr_config.and_then(|cfg| serde_json::to_value(&cfg).ok());
    let snap = ConfigSnapshot {
        wifi_list: &setting.wifi_list,
        server_url: &setting.server_url,
        asr_config,
        mic_model: setting.mic_model,
        prefer_builtin_asr: setting.prefer_builtin_asr,
    };
    serde_json::to_string(&snap).ok()
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
    pub atlas_png: (Vec<u8>, bool), // 主题 atlas(setup.html 上传);(data, ended)
    pub mic_model: u8,
    /// 键盘模式下是否优先用内置 ASR(Whisper);false 时 MIC 透传给主机(触发主机自带听写)。
    pub prefer_builtin_asr: bool,
    // TODO: 把 asr_config 挪进来(内存持有,启动从 NVS 加载、CONFIG 写入时双写),
    // config_snapshot_json 就不用再拿 nvs 现场读了。
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
        nvs.remove("atlas_png")?;
        nvs.remove("background_png")?; // 旧版背景图键,顺手清理
        nvs.remove("mic_model")?;
        nvs.remove(PREFER_BUILTIN_ASR_KEY)?;
        nvs.remove("state")?;
        // ASR 配置(uri/api_key/model)存在独立 NVS 键 audio::AsrConfig 管理,
        // 不归 Setting 管,但 Clear Config 应一并清除,否则残留导致下次开机仍读回旧 ASR。
        nvs.remove("asr_config")?;
        // Tailscale auth key 同理:独立键,一并在 Clear Config 里清除。
        nvs.remove(crate::tailscale::NVS_KEY)?;
        Ok(())
    }

    /// 擦掉 NimBLE 存在 NVS 里的绑定数据(所有已配对主机的 bond 密钥 + CCCD 订阅记录),
    /// 重启后生效 —— 各主机下次连接需要重新配对。
    /// ESP-IDF 的 NimBLE port(ble_store_nvs.c)把 bond 和 CCCD 都存在 "nimble_bond"
    /// 一个 namespace 里(NIMBLE_NVS_NAMESPACE);上游版另有 "nimble_cccd",一并尝试、
    /// 不存在即忽略。只能在设置菜单里调用:此时 NimBLE host 尚未启动(BLEDevice::take
    /// 在 boot-menu 分支之后),直接擦 NVS 是安全的;需要 host 在线的 ble_store_clear()
    /// 在这里反而用不了。
    pub fn clear_ble_bonds() {
        for ns in ["nimble_bond\0", "nimble_cccd\0"] {
            unsafe {
                let mut handle: esp_idf_svc::sys::nvs_handle_t = 0;
                let rc = esp_idf_svc::sys::nvs_open(
                    ns.as_ptr().cast(),
                    esp_idf_svc::sys::nvs_open_mode_t_NVS_READWRITE,
                    &mut handle,
                );
                if rc != 0 {
                    // namespace 不存在 = 本来就没有绑定记录,忽略。
                    continue;
                }
                esp_idf_svc::sys::nvs_erase_all(handle);
                esp_idf_svc::sys::nvs_commit(handle);
                esp_idf_svc::sys::nvs_close(handle);
            }
        }
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

        // 主题 atlas(setup.html 上传,202×54 由网页侧校验;设备侧不校验,
        // 解码失败由 atlas::init 回退内置主题)。键不存在则为空。
        let atlas_png_size = nvs
            .blob_len("atlas_png")
            .map_err(|e| log::error!("Failed to get atlas_png size: {:?}", e))
            .ok()
            .flatten()
            .unwrap_or(0);

        let atlas_png = if atlas_png_size != 0 {
            log::info!("Atlas PNG size in NVS: {} bytes", atlas_png_size);
            let mut buf = vec![0; atlas_png_size];
            match nvs.get_blob("atlas_png", &mut buf)? {
                Some(data) => data.to_vec(),
                None => Vec::new(),
            }
        } else {
            log::info!("No atlas PNG in NVS, using built-in theme.");
            Vec::new()
        };

        let state = nvs.get_u8("state")?.unwrap_or(0);
        nvs.set_u8("state", 0)?;

        let mic_model = nvs.get_u8("mic_model")?.unwrap_or(1);
        let prefer_builtin_asr = nvs.get_u8(PREFER_BUILTIN_ASR_KEY)?.unwrap_or(1) != 0;

        Ok(Setting {
            wifi_list,
            server_url,
            atlas_png: (atlas_png, false),
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
    /// 配网页写入了 tailscale_key:已存 NVS + 清旧 tailnet 注册,跳 Tailscale 页面。
    Tailscale,
    /// CONFIG 被写入:主循环里刷新特征值缓存。⚠️ 不能在 on_write 回调内直接做 ——
    /// handle_gap_event 分发回调期间持有特征自身的锁,重入 lock 即自死锁
    /// (栈任务冻死 → ATT 超时断链);经此事件到主循环任务再刷新就安全了。
    RefreshConfig,
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
    // 刷新缓存(BTevent::RefreshConfig → 主循环 → refresh_config_cache)要用。
    let _ = CONFIG_CHAR.set(config_characteristic.clone());
    config_characteristic
        .lock()
        .on_read(move |c, _| {
            // 读一次返回整份快照,免去多次读。注意:快照超过 MTU-3 时本回调不保证
            // 被触发 —— esp32-nimble 对长值的普通 Read Request 不调 on_read,直接
            // 回上次 set_value 的缓存;所以 on_write 末尾会主动刷新缓存。
            let setting = config_r.lock().unwrap();
            match config_snapshot_json(&setting.0, &setting.1) {
                Some(json) => {
                    log::info!("Config read: {} bytes", json.len());
                    c.set_value(json.as_bytes());
                }
                None => log::error!("Failed to serialize config snapshot"),
            }
        })
        .on_write({
            let evt_tx = evt_tx.clone();
            move |args| {
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
                    if let (Some(base_obj), Some(in_obj)) = (base.as_object_mut(), asr.as_object())
                    {
                        for (k, v) in in_obj {
                            // null = 未提供:vibeup 会把未填字段发成 null,跳过以保留
                            // 现有值(否则覆盖成 null 后反序列化失败,整个 merge 丢弃)。
                            if v.is_null() {
                                continue;
                            }
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

                if let Some(key) = save.tailscale_key {
                    // 空串忽略(页面上未填就保存其它项时,不该把已绑定的 key 冲掉)。
                    if !key.trim().is_empty() {
                        // 对齐 vibewatch:写入新 key 时擦掉 microlink 的旧 tailnet 注册
                        // (机器 key / WG key / 缓存 peers),否则新 key 会被旧注册顶掉。
                        // 此时没有任何 microlink 实例在跑(factory_reset 要求 init 前调用)。
                        unsafe {
                            let rc = esp_idf_svc::sys::microlink::microlink_factory_reset();
                            if rc != esp_idf_svc::sys::ESP_OK {
                                log::warn!("microlink factory reset failed: {rc}");
                            }
                        }
                        if let Err(e) = setting.1.set_str(crate::tailscale::NVS_KEY, key.trim()) {
                            log::error!("Failed to save tailscale_key: {:?}", e);
                        } else {
                            log::info!("tailscale_key saved ({} chars)", key.trim().len());
                        }
                        // 通知 keyboard 主循环跳 Tailscale 页面(连接 + 节点列表)。
                        if let Some(tx) = &evt_tx {
                            let _ = tx.blocking_send(BTevent::Tailscale);
                        }
                    }
                }

                // 写入改了配置:通知主循环刷新特征值缓存 —— esp32-nimble 对长值
                // (>MTU-3)的普通 Read Request 不触发 on_read、直接回缓存,不刷新
                // 客户端就读到旧值。刷新动作走 BTevent::RefreshConfig(为什么不在
                // 这里直接做,见该变体的注释)。
                if let Some(tx) = &evt_tx {
                    let _ = tx.blocking_send(BTevent::RefreshConfig);
                }
            }
        });

    let setting_gif = setting.clone();

    let background_png_characteristic =
        service.create_characteristic(BACKGROUND_PNG_ID, NimbleProperties::WRITE);
    background_png_characteristic.lock().on_write(move |args| {
        let gif_chunk = args.recv_data();

        if gif_chunk.len() <= 1024 * 1024 && gif_chunk.len() > 0 {
            log::info!("New atlas theme chunk received, size: {}", gif_chunk.len());
            let mut setting = setting_gif.lock().unwrap();
            // 上一次传输已结束(或 Clear 的 1 字节占位)再写 → 开新一轮,清掉旧累积
            if setting.0.atlas_png.1 {
                setting.0.atlas_png.0.clear();
                setting.0.atlas_png.1 = false;
            }
            setting.0.atlas_png.0.extend_from_slice(gif_chunk);
            if gif_chunk.len() < 512 {
                setting.0.atlas_png.1 = true; // Mark as valid
            }
            if setting.0.atlas_png.0.len() > 1024 * 1024 {
                log::warn!("Atlas theme size exceeds 1024KB, resetting to default.");
                setting.0.atlas_png.0.clear();
                setting.0.atlas_png.1 = false;
                args.reject();
            }
        } else {
            log::error!("Failed to parse new atlas theme chunk.");
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

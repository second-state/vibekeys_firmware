//! microlink(Tailscale)会话拉起:C 组件的 Rust 薄封装。
//! `start_and_wait` 同步轮询状态机直到 CONNECTED,状态经 `show` 回调上屏
//! (vibekeys 的 remote 入口在同步上下文里渲染,与 vibewatch 的 async gui 版不同)。
//! 隧道是透明 TUN 网口:连上后 MQTT 代码零改动,流量自动走 WireGuard。

use esp_idf_svc::sys::microlink as ml;

/// 上报给 tailnet 的设备名。
pub const DEVICE_NAME: &str = "vibekeys";

/// 把 microlink C 组件的各 TAG 日志压到 WARN:默认级别下 WG 握手/UDP 包 dump
/// (ml_wg_mgr 的 ESP_LOGI)每几秒刷一屏。仍保留 WARN/ERROR。
/// 在 microlink init/start 之后调用(用 esp_log_level_get 回读确认生效)。
pub fn quiet_logs() {
    use esp_idf_svc::sys::{esp_log_level_get, esp_log_level_set, esp_log_level_t_ESP_LOG_WARN};
    for tag in [
        "ml_wg_mgr",
        "ml_derp",
        "ml_net_io",
        "ml_coord",
        "ml_h2",
        "ml_stun",
        "ml_cell",
        "ml_at_sock",
        "ml_config",
        "ml_peer_nvs",
        "microlink",
    ] {
        // SAFETY: 纯运行时日志级别设置/回读;tag 经 CString 补 NUL——
        // 直接 tag.as_ptr() 传 &str 是无 NUL 结尾的,会读到内存垃圾导致 TAG 永远匹配不上。
        let ctag = std::ffi::CString::new(tag).unwrap();
        unsafe {
            esp_log_level_set(ctag.as_ptr(), esp_log_level_t_ESP_LOG_WARN);
            log::info!(
                "ml log: {tag} level = {} (WARN = {})",
                esp_log_level_get(ctag.as_ptr()),
                esp_log_level_t_ESP_LOG_WARN
            );
        }
    }
}

/// 会话结束时停止并释放 microlink 实例。
/// CString 保活:`microlink_init` 只浅拷贝 config(指针),auth_key/device_name
/// 字符串必须活过整个会话,否则每次 Hostinfo 上报都读已释放内存。
/// 字符串本身不再被读,留着的意义就是保活。
#[allow(dead_code)] // 字段 1:仅为指针保活,故意不读
pub struct MicrolinkGuard(pub *mut ml::microlink_s, [std::ffi::CString; 2]);

impl Drop for MicrolinkGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: handle 来自 microlink_init,stop 前一直有效。
            unsafe {
                ml::microlink_stop(self.0);
                ml::microlink_destroy(self.0);
            }
        }
    }
}

impl MicrolinkGuard {
    /// 向拥有 `ip`("A.B.C.D")的对端踢一次 WireGuard 握手:
    /// 隧道未建立时先于应用流量调用;可重复调用。IP 解析失败返回 ESP_FAIL。
    pub fn trigger_handshake(&self, ip: &str) -> esp_idf_svc::sys::esp_err_t {
        let Ok(ip) = std::ffi::CString::new(ip) else {
            return esp_idf_svc::sys::ESP_FAIL;
        };
        // SAFETY: ip 是合法的 NUL 结尾字符串。
        let vpn_ip = unsafe { ml::microlink_parse_ip(ip.as_ptr()) };
        if vpn_ip == 0 {
            return esp_idf_svc::sys::ESP_FAIL;
        }
        // SAFETY: microlink_init 返回的合法 handle。
        unsafe { ml::microlink_trigger_handshake(self.0, vpn_ip) }
    }

    /// 把我们的 DERP mailbox 搬到拥有 `ip` 的对端的 home region:
    /// microlink 的中继包只经我们自己的 mailbox 投递,对端在别的 region 时
    /// 不搬就不可达。返回搬过去的 region;对端(或其 region)未知返回 None。
    pub fn rehome_to_peer(&self, ip: &str) -> Option<u16> {
        let Ok(ip) = std::ffi::CString::new(ip) else {
            return None;
        };
        // SAFETY: ip 是合法的 NUL 结尾字符串。
        let vpn_ip = unsafe { ml::microlink_parse_ip(ip.as_ptr()) };
        if vpn_ip == 0 {
            return None;
        }
        // SAFETY: microlink_init 返回的合法 handle。
        let region = unsafe { ml::microlink_get_peer_home_region(self.0, vpn_ip) };
        if region == 0 {
            return None;
        }
        unsafe { ml::microlink_rehome_derp(self.0, region) };
        Some(region)
    }
}

/// tailnet 上本账号的一个节点(不含自己)。`online` = 当前可达。
#[derive(Debug, Clone)]
pub struct TailnetNode {
    pub hostname: String,
    #[allow(dead_code)] // 节点数据的一部分;页面暂只显示 hostname/online
    pub vpn_ip: std::net::Ipv4Addr,
    pub online: bool,
}

/// 枚举协调服务器告知的本账号全部节点,在线的排前面(Tailscale 页面列表用)。
pub fn tailnet_nodes(handle: &MicrolinkGuard) -> Vec<TailnetNode> {
    // SAFETY: microlink_init 返回的合法 handle。
    let count = unsafe { ml::microlink_get_peer_count(handle.0) }.max(0);
    let mut nodes: Vec<TailnetNode> = Vec::with_capacity(count as usize);
    for index in 0..count {
        let mut info = ml::microlink_peer_info_t {
            vpn_ip: 0,
            hostname: [0; 64],
            public_key: [0; 32],
            online: false,
            direct_path: false,
        };
        // SAFETY: 合法 handle 且 index 在 peer count 内。
        if unsafe { ml::microlink_get_peer_info(handle.0, index, &mut info) }
            == esp_idf_svc::sys::ESP_OK
        {
            let hostname = std::ffi::CStr::from_bytes_until_nul(&info.hostname)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            nodes.push(TailnetNode {
                hostname,
                vpn_ip: std::net::Ipv4Addr::from(info.vpn_ip),
                online: info.online,
            });
        }
    }
    nodes.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.hostname.cmp(&b.hostname))
    });
    nodes
}

/// 轮询 microlink 状态机的间隔。
const STATE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

fn state_label(state: ml::microlink_state_t) -> &'static str {
    match state {
        ml::microlink_state_t_ML_STATE_IDLE => "Idle",
        ml::microlink_state_t_ML_STATE_WIFI_WAIT => "Waiting for WiFi",
        ml::microlink_state_t_ML_STATE_CONNECTING => "Connecting",
        ml::microlink_state_t_ML_STATE_REGISTERING => "Registering",
        ml::microlink_state_t_ML_STATE_CONNECTED => "Connected",
        ml::microlink_state_t_ML_STATE_RECONNECTING => "Reconnecting",
        _ => "Error",
    }
}

/// 初始化 microlink 并等待注册完成(同步)。WiFi 必须已连接。
/// 状态变化经 `show(label)` 上屏;CONNECTED 返回会话 guard(调用方须持有到
/// 整个 remote 会话结束——drop 即拆隧道);失败/超时显示原因后返回 None。
pub fn start_and_wait(
    auth_key: &str,
    timeout: std::time::Duration,
    mut show: impl FnMut(&str),
) -> Option<MicrolinkGuard> {
    let Ok(auth_key) = std::ffi::CString::new(auth_key) else {
        show("Bad auth key");
        return None;
    };
    let device_name = std::ffi::CString::new(DEVICE_NAME).unwrap();
    let config = ml::microlink_config_t {
        auth_key: auth_key.as_ptr(),
        device_name: device_name.as_ptr(),
        enable_derp: true,
        enable_stun: true,
        enable_disco: true,
        max_peers: 8,
        wifi_tx_power_dbm: 0,
        priority_peer_ip: 0,
        disco_heartbeat_ms: 0,
        stun_interval_ms: 0,
        ctrl_watchdog_ms: 0,
    };
    // SAFETY: config 被 microlink_init 浅拷贝(仅指针),CString 在 guard 内保活整个会话。
    let handle = MicrolinkGuard(
        unsafe { ml::microlink_init(&config) },
        [auth_key, device_name],
    );
    if handle.0.is_null() {
        show("Init failed");
        std::thread::sleep(std::time::Duration::from_secs(2));
        return None;
    }

    // SAFETY: microlink_init 返回的合法 handle;WiFi 已连接。
    if unsafe { ml::microlink_start(handle.0) } != esp_idf_svc::sys::ESP_OK {
        show("Start failed");
        std::thread::sleep(std::time::Duration::from_secs(2));
        return None;
    }

    // init/start 之后静音组件日志(每次会话重新设一遍,防任何路径重置级别)。
    quiet_logs();

    // 轮询注册状态,变化时上屏。
    let started = std::time::Instant::now();
    let mut last_label = String::new();
    loop {
        let state = unsafe { ml::microlink_get_state(handle.0) };
        match state {
            ml::microlink_state_t_ML_STATE_CONNECTED => return Some(handle),
            ml::microlink_state_t_ML_STATE_ERROR => break,
            _ => {}
        }
        if started.elapsed() > timeout {
            break;
        }
        let label = state_label(state);
        if label != last_label {
            show(label);
            last_label = label.to_string();
        }
        std::thread::sleep(STATE_POLL_INTERVAL);
    }

    show("Connection failed");
    std::thread::sleep(std::time::Duration::from_secs(2));
    None
}

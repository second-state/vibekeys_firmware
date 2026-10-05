//! Tailscale 接入:microlink 组件的 NVS 密钥管理。
//! auth key(`tskey-auth-...`)经 BLE 配网页(setup.html)写入,存 NVS;
//! remote 模式发现 broker 位于 tailnet(100.64/10)时用它拉起 microlink 隧道。
//! 与 vibewatch_firmware 同款方案。

use esp_idf_svc::nvs::EspDefaultNvs;

/// NVS key(setting 命名空间)存 tailnet auth key。
pub(crate) const NVS_KEY: &str = "tailscale_key";

/// 读取已配置的 tailnet auth key,未配置返回 None。
pub(crate) fn stored_auth_key(nvs: &EspDefaultNvs) -> Option<String> {
    // str_len 含 NUL 结尾;get_str 返回不含 NUL 的字符串,以返回值为准。
    let len = nvs.str_len(NVS_KEY).ok()??;
    if len <= 1 {
        return None;
    }
    let mut buffer = vec![0u8; len];
    nvs.get_str(NVS_KEY, &mut buffer)
        .ok()?
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

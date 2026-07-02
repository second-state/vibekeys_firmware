use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    wifi::{AuthMethod, BlockingWifi, EspWifi},
};

pub fn connect(
    esp_wifi: &mut EspWifi<'static>,
    ssid: &str,
    pass: &str,
    sysloop: EspSystemEventLoop,
) -> anyhow::Result<()> {
    let mut auth_method = AuthMethod::WPA2Personal;
    if ssid.is_empty() {
        anyhow::bail!("Missing WiFi name")
    }
    if pass.is_empty() {
        auth_method = AuthMethod::None;
        log::info!("Wifi password is empty");
    }

    let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop)?;

    wifi.set_configuration(&esp_idf_svc::wifi::Configuration::Client(
        esp_idf_svc::wifi::ClientConfiguration {
            ssid: ssid
                .try_into()
                .expect("Could not parse the given SSID into WiFi config"),
            password: pass
                .try_into()
                .expect("Could not parse the given password into WiFi config"),
            auth_method,
            ..Default::default()
        },
    ))?;

    wifi.start()?;

    log::info!("Connecting wifi...");

    wifi.connect()?;

    log::info!("Waiting for DHCP lease...");

    wifi.wait_netif_up()?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    log::info!("Wifi DHCP info: {:?}", ip_info);

    Ok(())
}

/// 扫描周围 WiFi,返回去重(保序)后的 ssid 列表。
pub fn scan(
    esp_wifi: &mut EspWifi<'static>,
    sysloop: esp_idf_svc::eventloop::EspSystemEventLoop,
) -> anyhow::Result<Vec<String>> {
    let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop)?;
    // scan 需要驱动已 start;若已 start 则忽略错误
    let _ = wifi.start();
    let results = wifi.scan()?;
    let mut seen = std::collections::HashSet::new();
    let mut ssids = Vec::new();
    for ap in results {
        let s = ap.ssid.as_str().to_string();
        if !s.is_empty() && seen.insert(s.clone()) {
            ssids.push(s);
        }
    }
    Ok(ssids)
}

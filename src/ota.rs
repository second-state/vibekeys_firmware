//! OTA 模式:主固件设置菜单进入,同进程跑 HTTP server,把新固件写到对面 OTA 分区后重启。
//! 两种更新来源:浏览器上传(`/ota` PUT)、从 GitHub release 拉最新(`DownloadLatest`)。
//! 复用主固件的 `crate::lcd` / `crate::wifi` / `crate::bt_wifi_mode`,不再像旧版那样
//! 独立成一个小二进制并复制一份 lcd/wifi 驱动。

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::reset::restart,
    http::server::{Configuration as HttpServerConf, EspHttpServer, Method},
    io::Write,
    ota::EspOta,
};

/// 从 GitHub release 拉最新固件的目标 URL。按硬件 feature 选资产:max2 用 max2 镜像,
/// 否则用 keys 镜像。repo 指向本项目自己的 release(L-jasmine/vibekeys_firmware)。
#[cfg(feature = "max2")]
pub const OTA_DOWNLOAD_URL: &str =
    "https://github.com/L-jasmine/vibekeys_firmware/releases/latest/download/vibekeys_max2_ota.bin";
#[cfg(not(feature = "max2"))]
pub const OTA_DOWNLOAD_URL: &str =
    "https://github.com/L-jasmine/vibekeys_firmware/releases/latest/download/vibekeys_ota.bin";

static OTA_INDEX_HTML: &str = include_str!("../assets/ota_index.html");

enum OtaEvent {
    DataChunk(Vec<u8>),
    Complete,
    DownloadLatest,
}

/// 进入 OTA 模式。复用调用方(main)已建好的 WiFi/显示/按钮。
///
/// - 先用 boot 阶段的 `scan_list` 与 `setting.wifi_list` 匹配连 WiFi;
/// - 起 HTTP server(上传 `/ota`、下载触发 `/ota/download`、页面 `/`);
/// - `ota_task` 在 worker 线程里写分区;
/// - 主线程轮询按钮:`accept` 触发 download-latest,`esc` 退出回 boot menu;
/// - 任一更新路径完成都在 worker 里 `restart()`;ESC 时干净关闭 server 让 worker 退出后返回。
pub fn run(
    target: &mut crate::lcd::FrameBuffer,
    accept_btn: &mut crate::AnyBtn,
    esc_btn: &mut crate::AnyBtn,
    wifi: &mut esp_idf_svc::wifi::EspWifi<'static>,
    sysloop: EspSystemEventLoop,
    scan_list: &[String],
    setting: &crate::bt_wifi_mode::Setting,
) -> anyhow::Result<()> {
    crate::lcd::display_text(target, "OTA Mode\n Connecting wifi", 0)?;

    // 合并后 OTA 复用主固件的 wifi 实例:若刚从 remote 过来,wifi 可能已经连上了,
    // 这时再调 wifi::connect 的 connect() 会因「已连接」报错。所以已连接就直接复用,
    // 没连才走 pick_cred + connect(与主固件 remote 一致)。
    if !wifi.is_connected().unwrap_or(false) {
        // 进 OTA 重新扫一次:boot 阶段的 scan_list 可能漏扫/已过期(开机首次扫描常漏 AP)。
        // 与旧 OTA 二进制一致——现扫现连。scan_list 形参留作 fallback。
        let fresh = crate::wifi::scan(wifi, sysloop.clone()).unwrap_or_else(|e| {
            log::warn!("OTA fresh scan failed: {e:?}, fallback to boot scan_list");
            scan_list.to_vec()
        });
        log::info!(
            "OTA: scan {} ssids, wifi_list {} creds",
            fresh.len(),
            setting.wifi_list.len()
        );
        let r = match crate::bt_wifi_mode::pick_cred(&fresh, &setting.wifi_list) {
            Some(c) => {
                log::info!("OTA: picked ssid={:?} pass_len={}", c.ssid, c.pass.len());
                crate::wifi::connect(wifi, &c.ssid, &c.pass, sysloop)
            }
            None => anyhow::Result::<()>::Err(anyhow::anyhow!(
                "no known network in range (scan {})",
                fresh.len()
            )),
        };
        if let Err(e) = r {
            log::error!("OTA wifi connect failed: {:?}", e);
        }
    }
    if !wifi.is_connected().unwrap_or(false) {
        crate::lcd::display_text(target, "OTA Mode\n Connect wifi Failed\n ESC to back", 0)?;
        wait_button_release(esc_btn);
        return Ok(());
    }

    let ip = wifi.sta_netif().get_ip_info()?.ip;
    log::info!("OTA: WiFi connected, IP {}", ip);
    crate::lcd::display_text(
        target,
        &format!(
            "OTA: http://{ip}\n Accept: download latest\n ESC: exit\n (or upload via browser)"
        ),
        0,
    )?;

    let (tx, rx) = std::sync::mpsc::channel::<OtaEvent>();
    let screen_tx = tx.clone();
    let http_server = ota_http_server(tx)?;
    let ota_worker = std::thread::Builder::new()
        .name("ota-worker".to_string())
        .stack_size(1024 * 24)
        .spawn(move || {
            if let Err(e) = ota_task(rx) {
                log::error!("OTA worker failed: {e:?}");
            }
        })?;

    // 轮询按钮:accept 触发下载最新;esc 退出回 boot menu。HTTP 上传通路始终在线。
    loop {
        if accept_btn.is_low() {
            wait_button_release(accept_btn);
            log::info!("OTA: accept pressed, downloading latest from release");
            crate::lcd::display_text(
                target,
                "OTA Mode\n Downloading latest...\n Device will reboot",
                0,
            )?;
            let _ = screen_tx.send(OtaEvent::DownloadLatest);
            break;
        }
        if esc_btn.is_low() {
            wait_button_release(esc_btn);
            log::info!("OTA: esc pressed, exiting to boot menu");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // 关闭所有 sender(http_server 持有 upload/download 的 clone,screen_tx 是我们的),
    // 让 worker 的 rx 收到关闭信号后干净退出(下载/上传路径则早已 restart,join 不会返回)。
    drop(screen_tx);
    drop(http_server);
    let _ = ota_worker.join();
    Ok(())
}

/// 等按钮松开 + 简单消抖(按下期间一直 is_low)。
fn wait_button_release(btn: &crate::AnyBtn) {
    while btn.is_low() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
}

fn ota_http_server(
    tx: std::sync::mpsc::Sender<OtaEvent>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&HttpServerConf {
        stack_size: 10240,
        ..Default::default()
    })?;

    let upload_tx = tx.clone();
    server.fn_handler("/ota", Method::Put, move |mut request| {
        let mut buf = vec![0u8; 4096];
        let mut total = 0usize;

        loop {
            let n = request.read(&mut buf).map_err(|e| {
                log::error!("Failed to read OTA body: {:?}", e);
                anyhow::anyhow!("Failed to read OTA body: {:?}", e)
            })?;
            total += n;
            if n == 0 {
                break;
            }
            upload_tx
                .send(OtaEvent::DataChunk(buf[..n].to_vec()))
                .map_err(|e| {
                    log::error!("OTA channel closed: {:?}", e);
                    anyhow::anyhow!("OTA channel closed: {:?}", e)
                })?;
        }

        upload_tx.send(OtaEvent::Complete).map_err(|e| {
            log::error!("OTA channel closed: {:?}", e);
            anyhow::anyhow!("OTA channel closed: {:?}", e)
        })?;

        let mut resp = request.into_ok_response()?;
        resp.write_all(format!("OTA received: {} bytes", total).as_bytes())?;
        Result::<(), anyhow::Error>::Ok(())
    })?;

    server.fn_handler("/ota/download", Method::Post, move |request| {
        tx.send(OtaEvent::DownloadLatest).map_err(|e| {
            log::error!("OTA channel closed: {:?}", e);
            anyhow::anyhow!("OTA channel closed: {:?}", e)
        })?;

        let mut resp = request.into_ok_response()?;
        resp.write_all(b"Download started. Device will reboot after OTA completes.")?;
        Result::<(), anyhow::Error>::Ok(())
    })?;

    server.fn_handler("/", Method::Get, |req| {
        let html = OTA_INDEX_HTML.replace("{{OTA_DOWNLOAD_URL}}", OTA_DOWNLOAD_URL);
        req.into_ok_response()?.write_all(html.as_bytes())?;
        Result::<(), anyhow::Error>::Ok(())
    })?;

    server.fn_handler("/favicon.ico", Method::Get, |req| {
        req.into_ok_response()?.write_all(&[])?;
        Result::<(), anyhow::Error>::Ok(())
    })?;

    Ok(server)
}

/// worker:按到达的事件分发。DataChunk/DownloadLatest 各自接管 rx 走完整流程并 restart;
/// rx 关闭(主线程退出 OTA 模式)时返回 Ok。
fn ota_task(rx: std::sync::mpsc::Receiver<OtaEvent>) -> anyhow::Result<()> {
    while let Ok(ev) = rx.recv() {
        match ev {
            OtaEvent::DataChunk(data) => return ota_write_upload(rx, data),
            OtaEvent::DownloadLatest => return ota_download_latest(),
            OtaEvent::Complete => {}
        }
    }
    Ok(())
}

/// 处理浏览器上传:把后续 chunk 顺序写进对面 OTA 分区,Complete 后切换启动槽并 restart。
fn ota_write_upload(
    rx: std::sync::mpsc::Receiver<OtaEvent>,
    first_chunk: Vec<u8>,
) -> anyhow::Result<()> {
    let mut ota = EspOta::new()?;
    ota.mark_running_slot_valid()?;

    let mut update = ota.initiate_update()?;
    log::info!("OTA upload first chunk: {} bytes", first_chunk.len());
    update.write(&first_chunk)?;

    while let Ok(ev) = rx.recv() {
        match ev {
            OtaEvent::DataChunk(data) => {
                log::info!("OTA chunk: {} bytes", data.len());
                update.write(&data)?;
            }
            OtaEvent::Complete => break,
            OtaEvent::DownloadLatest => {
                log::warn!("Ignoring download request while upload OTA is active");
            }
        }
    }
    update.complete()?;
    log::info!("OTA upload complete, restarting into new firmware");
    restart();
}

/// 从 GitHub release 下载最新固件写进对面分区。带 content-length 时按已知尺寸擦写,
/// 否则擦整分区。
fn ota_download_latest() -> anyhow::Result<()> {
    log::info!("OTA download latest from {}", OTA_DOWNLOAD_URL);

    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(16 * 1024),
        buffer_size_tx: Some(1024),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(std::time::Duration::from_secs(60)),
        ..Default::default()
    };
    let conn = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
    let mut client = embedded_svc::http::client::Client::wrap(conn);
    let request = client.get(OTA_DOWNLOAD_URL)?;
    let mut response = request.submit()?;
    let status = response.status();
    log::info!("OTA download HTTP status: {}", status);
    if status != 200 {
        anyhow::bail!("OTA download failed: HTTP {}", status);
    }

    let content_len = response
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok());

    let mut ota = EspOta::new()?;
    ota.mark_running_slot_valid()?;
    let mut update = match content_len {
        Some(len) => {
            log::info!("OTA download content-length: {} bytes", len);
            ota.initiate_update_with_known_size(len)?
        }
        None => {
            log::warn!("OTA download missing content-length; erasing full OTA partition");
            ota.initiate_update()?
        }
    };

    let mut buf = vec![0u8; 8192];
    let mut total = 0usize;
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        update.write(&buf[..n])?;
        total += n;
        log::info!("OTA download chunk: {} bytes, total {}", n, total);
    }

    update.complete()?;
    log::info!("OTA download complete: {} bytes, restarting", total);
    restart();
}

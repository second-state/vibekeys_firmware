//! OTA 模式:主固件设置菜单进入,同进程跑 HTTP server,把新固件写到对面 OTA 分区后重启。
//! 两种更新来源:浏览器上传(`/ota` PUT)、从 GitHub release 拉最新(`DownloadLatest`)。
//! 复用主固件的 `crate::lcd` / `crate::wifi` / `crate::bt_wifi_mode`,不再像旧版那样
//! 独立成一个小二进制并复制一份 lcd/wifi 驱动。

use embedded_graphics::prelude::WebColors;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::reset::restart,
    http::server::{Configuration as HttpServerConf, EspHttpServer, Method},
    io::Write,
    ota::EspOta,
};

/// 从 GitHub release 拉最新固件的目标 URL。
///
/// 默认指向本项目(second-state/vibekeys_firmware)的 `releases/latest`(稳定版)。CI 构建
/// 预发布(rc/beta)时,通过环境变量 `VIBEKEYS_OTA_URL` 覆盖成该 tag 的具体资产 URL——因为
/// GitHub 的 `releases/latest` 排除 prerelease,prerelease 必须钉死 tag 才能下到。
/// 资产按硬件 feature 选:max2 → `vibekeys_max2_ota.bin`,否则 → `vibekeys_ota.bin`。
#[cfg(feature = "max2")]
const DEFAULT_OTA_URL: &str = "https://github.com/second-state/vibekeys_firmware/releases/latest/download/vibekeys_max2_ota.bin";
#[cfg(not(feature = "max2"))]
const DEFAULT_OTA_URL: &str =
    "https://github.com/second-state/vibekeys_firmware/releases/latest/download/vibekeys_ota.bin";

pub const OTA_DOWNLOAD_URL: &str = match option_env!("VIBEKEYS_OTA_URL") {
    Some(url) => url,
    None => DEFAULT_OTA_URL,
};

static OTA_INDEX_HTML: &str = include_str!("../assets/ota_index.html");

enum OtaEvent {
    DataChunk(Vec<u8>),
    Complete,
    DownloadLatest,
}

/// OTA 只读输入(scan_list + setting)打包成一个 struct 按引用传,避免 ota::run 参数过多
/// 触发 Xtensa codegen bug(最后一个栈参数 setting 被传成 null)。打包后 ota::run 共 6 个
/// 参数,全进寄存器(a2-a7),不压栈。
pub struct OtaData<'a> {
    pub scan_list: &'a Vec<String>,
    pub setting: &'a crate::bt_wifi_mode::Setting,
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
    data: &OtaData,
) -> anyhow::Result<()> {
    let scan_list = data.scan_list;
    let setting = data.setting;

    crate::lcd::display_text(target, "OTA Mode\n Connecting wifi", 0)?;

    // 合并后 OTA 复用主固件的 wifi 实例:若刚从 remote 过来,wifi 可能已经连上了,
    // 这时再调 wifi::connect 的 connect() 会因「已连接」报错。所以已连接就直接复用,
    // 没连才走 pick_cred + connect(与主固件 remote 一致)。
    // 直接复用 boot 阶段的 scan_list(remote 也用它):在已扫描过的 wifi 上再做一次
    // wifi::scan 会触发驱动空指针崩溃(第二次 scan 状态不稳),故不再重扫。
    if !wifi.is_connected().unwrap_or(false) {
        log::info!(
            "OTA: scan_list={} ssids, wifi_list={} creds",
            scan_list.len(),
            setting.wifi_list.len()
        );
        let r = match crate::bt_wifi_mode::pick_cred(scan_list.as_slice(), &setting.wifi_list) {
            Some(c) => {
                log::info!("OTA: picked ssid={:?} pass_len={}", c.ssid, c.pass.len());
                crate::wifi::connect(wifi, &c.ssid, &c.pass, sysloop)
            }
            None => anyhow::Result::<()>::Err(anyhow::anyhow!(
                "no known network in range (scan {})",
                scan_list.len()
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

    // 同步时间:OTA download-latest 走 HTTPS(TLS),证书校验依赖正确时间。
    // WiFi 已连上,可以 NTP。失败不阻塞——HTTP 上传不需要 TLS,download-latest 才需要。
    crate::lcd::display_text(target, "OTA Mode\n Syncing time...", 0)?;
    if let Err(e) = crate::sync_time(target) {
        log::warn!(
            "OTA: time sync failed (download-latest may fail TLS): {:?}",
            e
        );
    }

    crate::lcd::display_text(
        target,
        &format!(
            "OTA: http://{ip}\n Accept: download latest\n ESC: exit\n (or upload via browser)"
        ),
        0,
    )?;

    let (tx, rx) = std::sync::mpsc::channel::<OtaEvent>();
    // 下载进度通道:worker 每跨过 5% 发一次百分比,主线程刷新到屏幕。
    // 顺带当诊断用:若下载中途崩溃重启,屏幕会停在最后一个百分比上。
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<u8>();
    let screen_tx = tx.clone();
    let http_server = ota_http_server(tx)?;
    let ota_worker = std::thread::Builder::new()
        .name("ota-worker".to_string())
        .stack_size(1024 * 24)
        .spawn(move || {
            if let Err(e) = ota_task(rx, progress_tx) {
                log::error!("OTA worker failed: {e:?}");
            }
        })?;

    // 轮询按钮:accept 触发下载最新;esc 退出回 boot menu。HTTP 上传通路始终在线。
    loop {
        if accept_btn.is_low() {
            wait_button_release(accept_btn);
            log::info!("OTA: accept pressed, downloading latest from release");
            let render_progress = |target: &mut crate::lcd::FrameBuffer, pct: u8| {
                // display_text 只画字模不清背景,连续刷进度帧会叠在上一帧上,
                // 数字叠成一团——先清屏再画。
                target.clear(crate::lcd::ColorFormat::CSS_BLACK)?;
                crate::lcd::display_text(
                    target,
                    &format!("OTA Mode\n Downloading latest... {pct}%\n Device will reboot"),
                    0,
                )
            };
            render_progress(target, 0)?;
            let _ = screen_tx.send(OtaEvent::DownloadLatest);
            // 阻塞等下载结束:成功则 worker 直接 restart(不会走到这循环退出);
            // 失败则通道关闭 → 跳出 → 回 boot menu。
            // 超时给足 20s:第一条进度要等整分区擦除+下载启动(数秒),超时返回 Err
            // 会让 while let 直接退出循环,后面就再也不刷屏了。
            while let Ok(pct) = progress_rx.recv_timeout(std::time::Duration::from_secs(20)) {
                let _ = render_progress(target, pct);
            }
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
fn ota_task(
    rx: std::sync::mpsc::Receiver<OtaEvent>,
    progress_tx: std::sync::mpsc::Sender<u8>,
) -> anyhow::Result<()> {
    while let Ok(ev) = rx.recv() {
        match ev {
            OtaEvent::DataChunk(data) => return ota_write_upload(rx, data),
            OtaEvent::DownloadLatest => return ota_download_latest(Some(&progress_tx)),
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

/// 从 GitHub release(或 VIBEKEYS_OTA_URL)下载最新固件写进对面分区,带断点续传。
///
/// 下载链路实测不稳(经端口映射/代理时连接会在中途中被掐断:esp_transport_read -1,
/// errno 128 ENOTCONN)。所以连接断了不丢已写数据——OTA 分区句柄保持打开,
/// 带着 `Range: bytes=<已收>-` 重连续传;EOF 时若已收字节 < 总长也按掉线处理。
/// `progress` 有总长时每跨过 5% 回报一次百分比(0-100)。
fn ota_download_latest(progress: Option<&std::sync::mpsc::Sender<u8>>) -> anyhow::Result<()> {
    log::info!("OTA download latest from {}", OTA_DOWNLOAD_URL);

    const MAX_ATTEMPTS: usize = 10;

    let mut ota = EspOta::new()?;
    ota.mark_running_slot_valid()?;
    // 尺寸未知:整分区擦写(续传轮拿到的 content-length 是剩余长度,不可靠,省去分支)。
    let mut update = ota.initiate_update()?;

    let mut total: usize = 0;
    let mut full_len: Option<usize> = None; // 总长,首轮 200 响应里拿;进度百分比用
    let mut next_report: u8 = 5;
    let mut buf = vec![0u8; 8192];
    let mut completed = false;

    for attempt in 1..=MAX_ATTEMPTS {
        // 已收 > 0 时带 Range 续传(Response 借用 client,连接建立只能内联在循环里)。
        let range_from = if total == 0 { None } else { Some(total) };
        if attempt > 1 {
            log::warn!(
                "OTA download retry #{attempt}, resuming at {} bytes via Range",
                total
            );
        }
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(16 * 1024),
            buffer_size_tx: Some(1024),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            timeout: Some(std::time::Duration::from_secs(60)),
            ..Default::default()
        };
        let conn = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        let mut client = embedded_svc::http::client::Client::wrap(conn);
        // range/headers 必须活到 response 用完(header 切片借它们,生命周期跟 request 相同)。
        let range = format!("bytes={}-", total);
        let headers = [("Range", range.as_str())];
        let mut response = match range_from {
            Some(_) => {
                let request = client.request(Method::Get, OTA_DOWNLOAD_URL, &headers)?;
                request.submit()?
            }
            None => client.get(OTA_DOWNLOAD_URL)?.submit()?,
        };
        let status = response.status();
        log::info!("OTA download HTTP status: {}", status);

        // 首轮要 200;续传轮服务器必须支持 Range(206),返回 200 说明它忽略了 Range,
        // 继续写会从头覆盖错位,直接报错。
        if status != 200 && !(status == 206 && range_from.is_some()) {
            anyhow::bail!("OTA download failed: HTTP {}", status);
        }
        if status == 200 {
            full_len = response
                .header("content-length")
                .and_then(|value| value.parse::<usize>().ok());
        }

        // true = 本轮正常读完(EOF);false = 中途掉线,需要续传
        let mut round_eof = true;
        loop {
            match response.read(&mut buf) {
                Ok(0) => break, // 本轮 EOF
                Ok(n) => {
                    update.write(&buf[..n])?;
                    total += n;
                    if let (Some(tx), Some(len)) = (progress, full_len) {
                        if len > 0 {
                            let pct = ((total as u64 * 100) / len as u64) as u8;
                            if pct >= next_report {
                                let _ = tx.send(pct);
                                next_report = pct.saturating_add(5);
                            }
                        }
                    }
                    log::info!("OTA download chunk: {} bytes, total {}", n, total);
                }
                Err(e) => {
                    log::warn!("OTA download connection lost at {total} bytes: {e:?}");
                    round_eof = false;
                    break;
                }
            }
        }

        // 完整读完(EOF 且达到总长,或服务器没给总长且 EOF)→ 收尾重启
        if round_eof && full_len.map_or(true, |len| total >= len) {
            completed = true;
            break;
        }

        drop(response);
        drop(client);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    if !completed {
        anyhow::bail!("OTA download incomplete after {MAX_ATTEMPTS} attempts: got {total} bytes");
    }
    update.complete()?;
    log::info!("OTA download complete: {} bytes, restarting", total);
    restart();
}

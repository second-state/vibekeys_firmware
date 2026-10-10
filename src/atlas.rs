//! 元素级位图 ATLAS:嵌入 atlas-green.png(202×54,含白框间隙与调色板色板条),
//! 启动时解码到 RAM,blit 按 ABI 偏移表从图内抠取图区域(带 alpha:透明像素跳过)。
//!
//! 布局规格见 docs/design/bitmap-plates-assets.html(偏移表 + 调色板槽序 ABI)。
//! 框线、边线、面板底、选中条底 = 纯矩形,由调用方 fill_rect 直出,不进 atlas。

use anyhow::Result;
use embedded_graphics::prelude::*;
use image::RgbaImage;
use std::sync::Mutex;

use crate::lcd::{ColorFormat, FrameBuffer};

const ATLAS_PNG: &[u8] = include_bytes!("../assets/atlas-green.png");

static ATLAS: Mutex<Option<RgbaImage>> = Mutex::new(None);

/// sprite 取图区域(与 docs/design/bitmap-plates-assets.html 偏移表一致)
pub mod sprite {
    pub const ICON_BT_ON: (u32, u32, u32, u32) = (1, 1, 10, 10);
    pub const ICON_BT_OFF: (u32, u32, u32, u32) = (13, 1, 10, 10);
    pub const ICON_WF_ON: (u32, u32, u32, u32) = (25, 1, 10, 10);
    pub const ICON_WF_OFF: (u32, u32, u32, u32) = (37, 1, 10, 10);
    /// 预留:会话状态标记暂用内置 wqy12 字符(● ○,见 ui2::Status::marker),
    /// 这两个槽位图里保留,将来要换形状(非圆点)时再启用。
    #[allow(dead_code)]
    pub const DOT_WORKING: (u32, u32, u32, u32) = (49, 1, 6, 6);
    #[allow(dead_code)]
    pub const DOT_IDLE: (u32, u32, u32, u32) = (57, 1, 6, 6);
    pub const LOGO_BOOT: (u32, u32, u32, u32) = (1, 13, 200, 16);
    pub const BOX_TL: (u32, u32, u32, u32) = (1, 31, 6, 6);
    pub const BOX_TR: (u32, u32, u32, u32) = (9, 31, 6, 6);
    pub const BOX_BL: (u32, u32, u32, u32) = (17, 31, 6, 6);
    pub const BOX_BR: (u32, u32, u32, u32) = (25, 31, 6, 6);
    pub const FOCUS_TL: (u32, u32, u32, u32) = (33, 31, 6, 6);
    pub const FOCUS_TR: (u32, u32, u32, u32) = (41, 31, 6, 6);
    pub const FOCUS_BL: (u32, u32, u32, u32) = (49, 31, 6, 6);
    pub const FOCUS_BR: (u32, u32, u32, u32) = (57, 31, 6, 6);
}

/// 启动时调用一次:优先用 NVS 里的主题(setup.html 上传的 "atlas_png" blob,
/// 202×54 由网页侧校验,设备侧不检查),没有或解码失败回退内置绿色主题。
pub fn init(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> Result<()> {
    let custom = read_nvs_atlas(nvs);
    let (bytes, source) = match custom {
        Some(b) => (b, "nvs"),
        None => (ATLAS_PNG.to_vec(), "built-in"),
    };
    match image::load_from_memory(&bytes) {
        Ok(img) => {
            let img = img.to_rgba8();
            log::info!(
                "atlas decoded ({}): {}x{} ({} bytes png)",
                source,
                img.width(),
                img.height(),
                bytes.len()
            );
            *ATLAS.lock().unwrap() = Some(img);
            Ok(())
        }
        Err(e) => {
            if source == "nvs" {
                log::error!("NVS atlas decode failed ({e:?}), falling back to built-in theme");
                return init_fallback();
            }
            Err(e.into())
        }
    }
}

fn init_fallback() -> Result<()> {
    let img = image::load_from_memory(ATLAS_PNG)?.to_rgba8();
    log::info!(
        "atlas decoded (built-in): {}x{} ({} bytes png)",
        img.width(),
        img.height(),
        ATLAS_PNG.len()
    );
    *ATLAS.lock().unwrap() = Some(img);
    Ok(())
}

/// 读 NVS "atlas_png" blob;不存在/为空返回 None
fn read_nvs_atlas(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> Option<Vec<u8>> {
    let size = nvs
        .blob_len("atlas_png")
        .map_err(|e| log::error!("atlas_png blob_len: {e:?}"))
        .ok()
        .flatten()
        .unwrap_or(0);
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    match nvs.get_blob("atlas_png", &mut buf) {
        Ok(Some(data)) => Some(data.to_vec()),
        other => {
            log::error!("atlas_png read failed: {other:?}");
            None
        }
    }
}

/// 调色板槽位的原始 RGB:色板条取图区 (1,49) 起 16 槽 × 4×4,取各槽中心像素。
/// 槽序 = ABI:BG/PANEL/BORDER/ACCENT/ACCENT_HI/TEXT/DIM/FAINT/
///           FOCUS_FILL/ONBAR/OK/PERM/IDLE/ERR/STOP/RSVD(ui2 的 PAL 表从这里读)
pub fn palette_rgb(idx: usize) -> (u8, u8, u8) {
    let guard = ATLAS.lock().unwrap();
    let img = guard.as_ref().expect("atlas not init");
    let p = img.get_pixel(1 + idx as u32 * 4 + 2, 49 + 2);
    (p[0], p[1], p[2])
}

/// 从 atlas 抠取图区域 blit 到屏幕(带 alpha:透明像素跳过,不遮背景)。
/// target = 当前持有的 FrameBuffer(与 ui::fill_rect 同模式)。
pub fn blit(
    target: &mut FrameBuffer,
    sx: u32,
    sy: u32,
    w: u32,
    h: u32,
    dx: i32,
    dy: i32,
) -> Result<()> {
    let guard = ATLAS.lock().unwrap();
    let img = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("atlas not init"))?;

    let mut pixels = Vec::with_capacity((w * h) as usize);
    for row in 0..h {
        for col in 0..w {
            let p = img.get_pixel(sx + col, sy + row);
            if p[3] < 128 {
                continue; // 透明像素跳过:不遮屏幕上的背景
            }
            pixels.push(Pixel(
                Point::new(dx + col as i32, dy + row as i32),
                rgb_to_cf(p[0], p[1], p[2]),
            ));
        }
    }
    target.draw_iter(pixels)?;
    Ok(())
}

fn rgb_to_cf(r: u8, g: u8, b: u8) -> ColorFormat {
    ColorFormat::new(
        r / (u8::MAX / ColorFormat::MAX_R),
        g / (u8::MAX / ColorFormat::MAX_G),
        b / (u8::MAX / ColorFormat::MAX_B),
    )
}

//! ui2 — 数据驱动 UI:一块屏 = 一张元素表,渲染器只负责走表。
//!
//! 与旧 ui.rs 的区别:屏幕内容(元素表)与绘制逻辑(渲染器)彻底分离。
//! 渲染器只会三件事:`fill_rect` 铺底/画线(颜色全部来自 atlas 色板条)、
//! `atlas::blit` 抠 sprite(图标/字标/角贴)、画文字。
//! 换主题 = 换 atlas.png(色板条随图走);改布局 = 改 builder 产出的元素表;
//! 渲染器本身不含任何屏幕知识。
//!
//! 版本号、会话列表、弹窗文案都是运行时数据,由 builder 填进元素表。
#![allow(dead_code)]

use embedded_graphics::{
    mono_font::{ascii::FONT_7X13_BOLD, MonoTextStyle},
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, StyledDrawable},
    text::LineHeight,
};
use embedded_text::{
    alignment::HorizontalAlignment, style::HeightMode, style::TextBoxStyleBuilder,
    style::VerticalOverdraw, TextBox,
};
use u8g2_fonts::{fonts::u8g2_font_wqy12_t_gb2312, U8g2TextStyle};

use crate::atlas;
use crate::lcd::{ColorFormat, FrameBuffer};
// flush() 所在 trait
use crate::lcd::DisplayTargetDrive;

const LINE_H: u32 = 14;

/* ============ 调色板(启动后从 atlas 色板条读出;槽序 = ABI,见 bitmap-plates-assets.html) ============ */
pub mod slot {
    pub const BG: usize = 0;
    pub const PANEL: usize = 1;
    pub const BORDER: usize = 2;
    pub const ACCENT: usize = 3;
    pub const ACCENT_HI: usize = 4;
    pub const TEXT: usize = 5;
    pub const DIM: usize = 6;
    pub const FAINT: usize = 7;
    pub const FOCUS_FILL: usize = 8;
    pub const ONBAR: usize = 9;
    pub const OK: usize = 10;
    pub const PERM: usize = 11;
    pub const IDLE: usize = 12;
    pub const ERR: usize = 13;
    pub const STOP: usize = 14;
    pub const RSVD: usize = 15;
}

static PAL: std::sync::Mutex<Option<[(u8, u8, u8); 16]>> = std::sync::Mutex::new(None);

/// 语义槽位 → 屏幕色(RGB565)
pub fn pal(idx: usize) -> ColorFormat {
    let (r, g, b) = pal_rgb(idx);
    ColorFormat::new(
        r / (u8::MAX / ColorFormat::MAX_R),
        g / (u8::MAX / ColorFormat::MAX_G),
        b / (u8::MAX / ColorFormat::MAX_B),
    )
}

pub fn pal_rgb(idx: usize) -> (u8, u8, u8) {
    let guard = PAL.lock().unwrap();
    guard.as_ref().map_or((0, 0, 0), |p| p[idx])
}

/// 从 atlas 色板条读全表(`atlas::init` 之后、首帧之前调用一次)
pub fn load_palette() {
    let mut p = [(0u8, 0u8, 0u8); 16];
    for (i, slot) in p.iter_mut().enumerate() {
        *slot = atlas::palette_rgb(i);
    }
    *PAL.lock().unwrap() = Some(p);
}

/* ============ 布局常量(双尺寸:max2 320×172 / 小屏 284×78;
小屏几何按设计稿 atlas-small-screen.html 定) ============ */
pub const SCREEN_W: i32 = crate::lcd::DISPLAY_WIDTH as i32;
pub const SCREEN_H: i32 = crate::lcd::DISPLAY_HEIGHT as i32;

#[cfg(feature = "max2")]
pub mod geo {
    pub const FRAME_X: i32 = 4;
    pub const FRAME_Y: i32 = 6;
    pub const FRAME_W: i32 = super::SCREEN_W - FRAME_X * 2; // 312
    pub const FRAME_H: i32 = super::SCREEN_H - FRAME_Y - 6; // y6..168
    pub const LIST_TOP: i32 = 24;
    pub const ROW_STEP: i32 = 19;
    pub const ROW_H: i32 = 16;
    // 标题嵌片:挖 11px、文字 rect 14px(7×13 字体)
    pub const TITLE_DY: i32 = -5;
    pub const TITLE_H: u32 = 11;
    pub const TITLE_TEXT_DY: i32 = -6;
    pub const TITLE_TEXT_H: u32 = 14;
    pub const ICON_WF_X: i32 = super::SCREEN_W - 34;
    pub const ICON_BT_X: i32 = super::SCREEN_W - 48;
    pub const ICON_Y: i32 = 2;
    // 标题嵌片跨顶线(挖 BG 底)
    pub const TITLE_INSIDE: bool = false;
}
#[cfg(not(feature = "max2"))]
pub mod geo {
    pub const FRAME_X: i32 = 2;
    pub const FRAME_Y: i32 = 6;
    pub const FRAME_W: i32 = super::SCREEN_W - FRAME_X * 2; // 280
    pub const FRAME_H: i32 = super::SCREEN_H - FRAME_Y - 4; // y6..74
    pub const LIST_TOP: i32 = 26; // 标题放框内,列表起点下移
    pub const ROW_STEP: i32 = 16;
    pub const ROW_H: i32 = 13; // 6px 角贴 6+6 < 13,不重叠
                               // 标题放框内(不跨顶线、不挖嵌片):文字 rect 13px,贴顶线下方
    pub const TITLE_INSIDE: bool = true;
    pub const TITLE_DY: i32 = 3;
    pub const TITLE_H: u32 = 0; // 不挖底
    pub const TITLE_TEXT_DY: i32 = 3;
    pub const TITLE_TEXT_H: u32 = 13;
    pub const ICON_WF_X: i32 = super::SCREEN_W - 26;
    pub const ICON_BT_X: i32 = super::SCREEN_W - 40;
    pub const ICON_Y: i32 = 2;
}
pub use geo::*;

/* ============ 数据模型 ============ */

/// 颜色语义(→ 调色板槽位)。元素只声明语义色,不写死 RGB。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Text,
    Dim,
    Accent,
    Ok,
    Perm,
    Idle,
    Err,
    Faint,
    OnBar,
}
impl Ink {
    pub fn slot(self) -> usize {
        match self {
            Ink::Text => slot::TEXT,
            Ink::Dim => slot::DIM,
            Ink::Accent => slot::ACCENT,
            Ink::Ok => slot::OK,
            Ink::Perm => slot::PERM,
            Ink::Idle => slot::IDLE,
            Ink::Err => slot::ERR,
            Ink::Faint => slot::FAINT,
            Ink::OnBar => slot::ONBAR,
        }
    }
    fn color(self) -> ColorFormat {
        pal(self.slot())
    }
}

/// 会话状态:决定行首标记字符与标记颜色
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Work,
    Perm,
    Err,
    Idle,
    Dim,
}
impl Status {
    /// 行首标记(形状将来可由 atlas 状态槽 sprite 替换,先用字符)
    pub fn marker(self) -> &'static str {
        match self {
            Status::Work => "●",
            Status::Perm => "▲",
            Status::Err => "×",
            Status::Idle => "◆",
            Status::Dim => "·",
        }
    }
    pub fn ink(self) -> Ink {
        match self {
            Status::Work => Ink::Ok,
            Status::Perm => Ink::Perm,
            Status::Err => Ink::Err,
            Status::Idle => Ink::Idle,
            Status::Dim => Ink::Faint,
        }
    }
}

/// 列表行
#[derive(Clone)]
pub struct Row {
    pub text: String,
    pub status: Status,
}
impl Row {
    pub fn new(text: impl Into<String>, status: Status) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

/// 元素:一块屏的组成单元
pub enum Elem {
    /// 屏幕外框(1px 边线,BORDER 色;渲染器自动画,无需进表)
    // (见 render 开头)
    /// 标题嵌片(固定画在框顶线上,挖 BG 底;center=true 时水平居中)
    Title { text: String, center: bool },
    /// 列表行(状态标记 + 文字;focus=true 时文字用 ONBAR,配 FocusBar 用)
    Row { y: i32, row: Row, focus: bool },
    /// 通栏焦点框:FOCUS_FILL 底 + 4 边 accent 线 + 4 角 sprite(线宽代码写死)
    FocusBar { y: i32 },
    /// 通用文字
    Text {
        x: i32,
        y: i32,
        text: String,
        ink: Ink,
        center: bool,
    },
    /// 右上状态图标(bt/wf 连接状态决定 blit 哪个 sprite)
    Icons { bt: bool, wf: bool },
    /// 字标 blit
    Logo { x: i32, y: i32 },
    /// 弹窗框:1px 边线 + 4 角 sprite(边线宽度代码写死,花样只从 atlas 取角)
    Box { x: i32, y: i32, w: i32, h: i32 },
    /// 多行段落(自动折行,高度伸到 footer 上沿;支持 ANSI 转义,ASR 编辑器正文用)
    Para {
        x: i32,
        y: i32,
        w: i32,
        text: String,
        ink: Ink,
    },
    /// footer 键位提示(左右各一段)
    Footer { left: String, right: String },
}

/* ============ 渲染器 ============ */

/// 渲染一块屏:铺底 → 外框 → 走元素表。
/// 蓝牙/WiFi 状态由表内的 `Elem::Icons{ bt, wf }` 携带,渲染器无全局状态。
pub fn render(elems: &[Elem], target: &mut FrameBuffer) -> anyhow::Result<()> {
    // 底色
    fill(
        target,
        Rectangle::new(Point::zero(), Size::new(SCREEN_W as u32, SCREEN_H as u32)),
        pal(slot::BG),
    )?;
    // 外框
    let f = Rectangle::new(
        Point::new(FRAME_X, FRAME_Y),
        Size::new(FRAME_W as u32, FRAME_H as u32),
    );
    // FRAME_* 由 geo 模块按屏幕尺寸给出
    stroke(target, f, pal(slot::BORDER))?;

    for elem in elems {
        match elem {
            Elem::Title { text, center } => {
                let w = text_w(text) + 10;
                let text = text.as_str();
                let tx = if *center {
                    (SCREEN_W - w) / 2
                } else {
                    FRAME_X + 8
                };
                // 小屏:标题放框内不跨线;max2:挖穿顶线做嵌片
                if !TITLE_INSIDE {
                    fill(
                        target,
                        Rectangle::new(
                            Point::new(tx, FRAME_Y + TITLE_DY),
                            Size::new(w as u32, TITLE_H),
                        ),
                        pal(slot::BG),
                    )?;
                }
                draw_str(
                    target,
                    text,
                    Rectangle::new(
                        Point::new(tx, FRAME_Y + TITLE_TEXT_DY),
                        Size::new(w as u32, TITLE_TEXT_H),
                    ),
                    pal(slot::ACCENT),
                    HorizontalAlignment::Center,
                )?;
            }
            Elem::FocusBar { y } => draw_focus_bar(target, *y)?,
            Elem::Row { y, row, focus } => draw_row(target, *y, row, *focus)?,
            Elem::Text {
                x,
                y,
                text,
                ink,
                center,
            } => {
                let (tx, rect) = if *center {
                    // 居中交给 TextBox(整屏宽的 rect),不再自己算 x(会双重居中)
                    (0, Size::new(SCREEN_W as u32, LINE_H))
                } else {
                    (*x, Size::new((SCREEN_W - *x - 4) as u32, LINE_H))
                };
                draw_str(
                    target,
                    text,
                    Rectangle::new(Point::new(tx, *y), rect),
                    ink.color(),
                    if *center {
                        HorizontalAlignment::Center
                    } else {
                        HorizontalAlignment::Left
                    },
                )?;
            }
            Elem::Icons { bt, wf } => {
                let (bt_s, wf_s) = match (*bt, *wf) {
                    (true, true) => (atlas::sprite::ICON_BT_ON, atlas::sprite::ICON_WF_ON),
                    (true, false) => (atlas::sprite::ICON_BT_ON, atlas::sprite::ICON_WF_OFF),
                    (false, true) => (atlas::sprite::ICON_BT_OFF, atlas::sprite::ICON_WF_ON),
                    (false, false) => (atlas::sprite::ICON_BT_OFF, atlas::sprite::ICON_WF_OFF),
                };
                let (sx, sy, w, h) = wf_s;
                atlas::blit(target, sx, sy, w, h, ICON_WF_X, ICON_Y)?;
                let (sx, sy, w, h) = bt_s;
                atlas::blit(target, sx, sy, w, h, ICON_BT_X, ICON_Y)?;
            }
            Elem::Logo { x, y } => {
                let (sx, sy, w, h) = atlas::sprite::LOGO_BOOT;
                atlas::blit(target, sx, sy, w, h, *x, *y)?;
            }
            Elem::Box { x, y, w, h } => {
                stroke(
                    target,
                    Rectangle::new(Point::new(*x, *y), Size::new(*w as u32, *h as u32)),
                    pal(slot::ACCENT),
                )?;
                // 四角花样 sprite 盖在角上
                let cs = 6;
                for (s, cx, cy) in [
                    (atlas::sprite::BOX_TL, *x, *y),
                    (atlas::sprite::BOX_TR, *x + *w - cs, *y),
                    (atlas::sprite::BOX_BL, *x, *y + *h - cs),
                    (atlas::sprite::BOX_BR, *x + *w - cs, *y + *h - cs),
                ] {
                    let (sx, sy, sw, sh) = s;
                    atlas::blit(target, sx, sy, sw, sh, cx, cy)?;
                }
            }
            Elem::Para { x, y, w, text, ink } => {
                let h = (SCREEN_H - 18 - *y).max(LINE_H as i32) as u32;
                draw_str(
                    target,
                    text,
                    Rectangle::new(Point::new(*x, *y), Size::new(*w as u32, h)),
                    ink.color(),
                    HorizontalAlignment::Left,
                )?;
            }
            Elem::Footer { left, right } => {
                // 设计稿格式:键名 ACCENT 高亮,说明 DIM(如 Accept=Send → Accept 亮、=Send 暗)。
                // 位置收回扩展后的框内:贴底边 4px。
                let fy = FRAME_Y + FRAME_H - LINE_H as i32 - 4;
                draw_footer(target, FRAME_X + 8, fy, left)?;
                if !right.is_empty() {
                    let rw = footer_w(right);
                    draw_footer(target, FRAME_X + FRAME_W - 8 - rw, fy, right)?;
                }
            }
        }
    }
    // 元素画进 FrameBuffer 后要推一次屏(与 ui.rs 各 render_* 的 flush 一致)
    target.flush()?;
    Ok(())
}

/* ---- footer 双色渲染(设计稿格式:键名 ACCENT、说明 DIM) ---- */

/// 整段宽度:逐词累加(含词间空格 7px)
fn footer_w(s: &str) -> i32 {
    s.split(' ')
        .filter(|w| !w.is_empty())
        .map(footer_word_w)
        .sum::<i32>()
        + s.split(' ').filter(|w| !w.is_empty()).count() as i32 * 7
        - 7
}

/// 单词宽度;"Key=Value" 拆两截算
fn footer_word_w(w: &str) -> i32 {
    match w.split_once('=') {
        Some((k, v)) => text_w(k) + text_w("=") + text_w(v),
        None => text_w(w),
    }
}

/// 逐词画 footer:含 '=' 的词,键名画 ACCENT、"=值" 画 DIM;普通词整体 DIM
fn draw_footer(target: &mut FrameBuffer, x: i32, y: i32, s: &str) -> anyhow::Result<()> {
    let mut cx = x;
    let mut first = true;
    for word in s.split(' ').filter(|w| !w.is_empty()) {
        if first {
            first = false;
        } else {
            cx += text_w(" ");
        }
        let rect = Rectangle::new(
            Point::new(cx, y),
            Size::new((SCREEN_W as u32).max(1), LINE_H),
        );
        match word.split_once('=') {
            Some((k, v)) => {
                draw_str(
                    target,
                    k,
                    rect,
                    pal(slot::ACCENT),
                    HorizontalAlignment::Left,
                )?;
                cx += text_w(k);
                let kv = format!("={v}");
                let rect = Rectangle::new(
                    Point::new(cx, y),
                    Size::new((SCREEN_W as u32).max(1), LINE_H),
                );
                draw_str(target, &kv, rect, pal(slot::DIM), HorizontalAlignment::Left)?;
                cx += text_w(&kv);
            }
            None => {
                draw_str(
                    target,
                    word,
                    rect,
                    pal(slot::DIM),
                    HorizontalAlignment::Left,
                )?;
                cx += text_w(word);
            }
        }
    }
    Ok(())
}

/* ---- 行 / 焦点框 ---- */
fn draw_row(target: &mut FrameBuffer, y: i32, row: &Row, focus: bool) -> anyhow::Result<()> {
    let x = FRAME_X + 12;
    // 标记:焦点行用 ONBAR(状态色画在焦点条底上会同色隐形),普通行用状态色
    draw_str(
        target,
        row.status.marker(),
        Rectangle::new(Point::new(x, y + 1), Size::new(12, LINE_H)),
        if focus {
            Ink::OnBar.color()
        } else {
            row.status.ink().color()
        },
        HorizontalAlignment::Left,
    )?;
    // 文本:焦点行 ONBAR,普通行 TEXT
    draw_str(
        target,
        &row.text,
        Rectangle::new(
            Point::new(x + 14, y + 1),
            Size::new((FRAME_W - 30) as u32, LINE_H),
        ),
        if focus {
            Ink::OnBar.color()
        } else {
            Ink::Text.color()
        },
        HorizontalAlignment::Left,
    )?;
    Ok(())
}

/// 焦点框:填充 + 4 边线(线宽 1,代码写死)+ 4 角 sprite(花样只从 atlas 取)
fn draw_focus_bar(target: &mut FrameBuffer, y: i32) -> anyhow::Result<()> {
    let fx = FRAME_X + 2;
    let fw = FRAME_W - 4;
    fill(
        target,
        Rectangle::new(Point::new(fx, y), Size::new(fw as u32, ROW_H as u32)),
        pal(slot::FOCUS_FILL),
    )?;
    stroke(
        target,
        Rectangle::new(Point::new(fx, y), Size::new(fw as u32, ROW_H as u32)),
        pal(slot::ACCENT),
    )?;
    for (s, cx, cy) in [
        (atlas::sprite::FOCUS_TL, fx, y),
        (atlas::sprite::FOCUS_TR, fx + fw - 6, y),
        (atlas::sprite::FOCUS_BL, fx, y + ROW_H - 6),
        (atlas::sprite::FOCUS_BR, fx + fw - 6, y + ROW_H - 6),
    ] {
        let (sx, sy, w, h) = s;
        atlas::blit(target, sx, sy, w, h, cx, cy)?;
    }
    Ok(())
}

/* ============ 场景构建器(数据驱动:屏 = 元素表) ============ */

/// BOOT:字标 + 版本 + 三项菜单(焦点由 focus 指定)
pub fn boot(version: &str, focus: usize) -> Vec<Elem> {
    let items = ["Keyboard", "Remote", "Setting"];
    let mut elems = Vec::new();
    #[cfg(feature = "max2")]
    {
        elems.push(Elem::Logo { x: 60, y: 16 });
        elems.push(Elem::Text {
            x: 0,
            y: 36,
            text: version.into(),
            ink: Ink::Faint,
            center: true,
        });
        for (i, item) in items.iter().enumerate() {
            let y = 58 + i as i32 * ROW_STEP;
            if i == focus {
                elems.push(Elem::FocusBar { y });
            }
            elems.push(Elem::Text {
                x: 0,
                y: y + 2,
                text: (*item).into(),
                ink: if i == focus { Ink::OnBar } else { Ink::Dim },
                center: true,
            });
        }
    }
    // 小屏:78px 放不下字标+版本+3 行菜单 → 标题嵌片 + 版本 + 单行选项
    // (只显示焦点项,NEXT 轮换)
    #[cfg(not(feature = "max2"))]
    {
        elems.push(Elem::Title {
            text: "VibeKeys".into(),
            center: true,
        });
        elems.push(Elem::Text {
            x: 0,
            y: 24,
            text: version.into(),
            ink: Ink::Faint,
            center: true,
        });
        let y = 44;
        elems.push(Elem::FocusBar { y });
        elems.push(Elem::Text {
            x: 0,
            y: y + 1,
            text: items[focus.min(items.len() - 1)].into(),
            ink: Ink::OnBar,
            center: true,
        });
    }
    elems
}

/// SETTING:四项菜单(focus 行走焦点框)
pub fn setting(focus: usize, wifi_count: usize) -> Vec<Elem> {
    let items = [
        format!("WiFi networks ({wifi_count})"),
        "OTA Update".to_string(),
        "Clear config".to_string(),
        "Clear BLE".to_string(),
        "Power off".to_string(),
        "Hardware test".to_string(),
    ];
    let mut elems = vec![
        Elem::Title {
            text: "Setting".into(),
            center: false,
        },
        Elem::Icons {
            bt: false,
            wf: false,
        },
    ];
    for (i, item) in items.iter().enumerate() {
        let y = LIST_TOP + i as i32 * ROW_STEP;
        if i == focus {
            elems.push(Elem::FocusBar { y });
        }
        elems.push(Elem::Text {
            x: FRAME_X + 14,
            y: y + 2,
            text: item.clone(),
            ink: if i == focus {
                Ink::OnBar
            } else if matches!(i, 2 | 3 | 4) {
                // 破坏性操作(清配置/清绑定/关机)标红提醒
                Ink::Err
            } else {
                Ink::Dim
            },
            center: false,
        });
    }
    elems
}

/// SESSIONS:动态会话行(需关注的排前,由调用方排好序再传入)
pub fn sessions(rows: &[Row], focus: usize) -> Vec<Elem> {
    let mut elems = vec![Elem::Title {
        text: "Focus session".into(),
        center: false,
    }];
    for (i, row) in rows.iter().enumerate() {
        let y = LIST_TOP + i as i32 * ROW_STEP;
        if i == focus {
            elems.push(Elem::FocusBar { y });
        }
        elems.push(Elem::Row {
            y,
            row: row.clone(),
            focus: i == focus,
        });
    }
    elems
}

/// POPUP:弹窗框 + 底下变暗的会话列表 + 居中消息
pub fn popup(msg: &str) -> Vec<Elem> {
    // 小屏空间小,只留一行会话;弹窗框几何按屏幕尺寸
    let mut elems = vec![
        Elem::Title {
            text: "Sessions".into(),
            center: false,
        },
        Elem::Row {
            y: LIST_TOP,
            row: Row::new("vibekeys (9d1)", Status::Work),
            focus: false,
        },
    ];
    if cfg!(feature = "max2") {
        elems.push(Elem::Row {
            y: 43,
            row: Row::new("docs (c08)", Status::Idle),
            focus: false,
        });
    }

    let (box_rect, msg_y) = if cfg!(feature = "max2") {
        ((60, 70, 200, 40), 82)
    } else {
        ((42, 44, 200, 24), 50)
    };
    elems.push(Elem::Box {
        x: box_rect.0,
        y: box_rect.1,
        w: box_rect.2,
        h: box_rect.3,
    });
    elems.push(Elem::Text {
        x: 0,
        y: msg_y,
        text: msg.into(),
        ink: Ink::Text,
        center: true,
    });
    elems
}
/// 通用列表:标题 + 滚动窗口 + 焦点条(行数超屏时整页跟随焦点,与旧 render_list 同策略)。
/// 列表项是纯文本行(· 标记 + TEXT 色文字)。
pub fn list(title: &str, items: &[String], focus: usize) -> Vec<Elem> {
    let list_top = LIST_TOP;
    let bottom = FRAME_Y + FRAME_H;
    let visible = (((bottom - list_top) / ROW_STEP) as usize).max(1);
    let start = focus.saturating_sub(visible.saturating_sub(1));
    let mut elems = vec![Elem::Title {
        text: title.into(),
        center: false,
    }];
    for (i, item) in items.iter().enumerate().skip(start).take(visible) {
        let y = list_top + (i - start) as i32 * ROW_STEP;
        if i == focus {
            elems.push(Elem::FocusBar { y });
        }
        elems.push(Elem::Row {
            y,
            row: Row::new(item, Status::Dim),
            focus: i == focus,
        });
    }
    elems
}

/* ============ compat:旧 ui.rs 各渲染函数的 ui2 出口(双尺寸) ============ */
/// 签名与旧函数一一对应;状态机不动,只换渲染。几何全部来自 geo 模块。
pub mod compat {
    use super::*;

    /// 旧 render_setting_menu
    pub fn setting_menu(
        target: &mut FrameBuffer,
        focus: usize,
        wifi_count: usize,
    ) -> anyhow::Result<()> {
        let elems = setting(focus, wifi_count);
        render(&elems, target)
    }

    /// 旧 render_list(标题 + 条目 + 焦点)
    pub fn list(
        target: &mut FrameBuffer,
        title: &str,
        items: &[String],
        focus: usize,
    ) -> anyhow::Result<()> {
        let elems = super::list(title, items, focus);
        render(&elems, target)
    }

    /// 旧 render_session_list(remote 会话页):(label, is_working) →
    /// ● Work(OK 色)/ ○ Idle(IDLE 色)状态行 + 焦点条,滚动窗口与 list 同策略。
    pub fn session_list(
        target: &mut FrameBuffer,
        title: &str,
        items: &[(String, bool)],
        focus: usize,
    ) -> anyhow::Result<()> {
        const LIST_TOP: i32 = 24;
        let bottom = FRAME_Y + FRAME_H;
        let visible = (((bottom - LIST_TOP) / ROW_STEP) as usize).max(1);
        let start = focus.saturating_sub(visible.saturating_sub(1));
        let mut elems = vec![Elem::Title {
            text: title.into(),
            center: false,
        }];
        for (i, (label, is_working)) in items.iter().enumerate().skip(start).take(visible) {
            let y = LIST_TOP + (i - start) as i32 * ROW_STEP;
            if i == focus {
                elems.push(Elem::FocusBar { y });
            }
            elems.push(Elem::Row {
                y,
                row: Row::new(
                    label,
                    if *is_working {
                        Status::Work
                    } else {
                        Status::Idle
                    },
                ),
                focus: i == focus,
            });
        }
        render(&elems, target)
    }

    /// 旧 render_password:标题 = ssid,密码行 + 块状光标 + 字符轮盘(中格焦点)
    pub fn password(
        target: &mut FrameBuffer,
        header: &str,
        password: &str,
        focus: usize,
    ) -> anyhow::Result<()> {
        const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let n = ((FRAME_W as u32) / 16).clamp(5, 11) as usize;
        let cell_w = FRAME_W as u32 / n as u32;
        let half = n / 2;
        let wheel_y: i32 = if cfg!(feature = "max2") { 44 } else { 38 };
        let cell_h: i32 = if cfg!(feature = "max2") { 20 } else { 16 };

        let mut elems = vec![
            Elem::Title {
                text: header.into(),
                center: false,
            },
            // 密码文本 + 末尾块状光标(光标用 Box 描边;宽高 ≥ 12 才容得下 6px 角贴)
            Elem::Text {
                x: FRAME_X + 10,
                y: 26,
                text: password.to_string(),
                ink: Ink::Text,
                center: false,
            },
            Elem::Box {
                x: FRAME_X + 10 + text_w(password) + 2,
                y: 23,
                w: 13,
                h: 17,
            },
        ];
        for k in 0..n {
            let idx = (focus + k + CHARSET.len() - half) % CHARSET.len();
            let ch = (CHARSET[idx] as char).to_string();
            let cell_x = FRAME_X + k as i32 * cell_w as i32;
            if idx == focus {
                // 焦点格:描边框(角贴只在这 4 个角,格宽 ≥ 12 无碍)
                elems.push(Elem::Box {
                    x: cell_x,
                    y: wheel_y - 2,
                    w: cell_w as i32,
                    h: cell_h,
                });
            }
            // 格内字符手动居中(Elem::Text 的 center 是整屏居中,不适合格子)
            let ch_x = cell_x + (cell_w as i32 - text_w(&ch)) / 2;
            elems.push(Elem::Text {
                x: ch_x,
                y: wheel_y + 2,
                text: ch,
                ink: if idx == focus { Ink::Accent } else { Ink::Dim },
                center: false,
            });
        }
        render(&elems, target)
    }

    /// 旧 render_keyboard_view:标题 + 状态图标 + 居中反馈文本
    pub fn keyboard_view(
        target: &mut FrameBuffer,
        wifi_on: bool,
        ble_on: bool,
        feedback: &str,
    ) -> anyhow::Result<()> {
        let mut elems = vec![
            Elem::Title {
                text: "Keyboard".into(),
                center: false,
            },
            Elem::Icons {
                bt: ble_on,
                wf: wifi_on,
            },
        ];
        if !feedback.is_empty() {
            elems.push(Elem::Text {
                x: 0,
                y: SCREEN_H / 2 - 7,
                text: feedback.into(),
                ink: Ink::Text,
                center: true,
            });
        }
        render(&elems, target)
    }

    /// 旧 render_remote_view:working 时实际画面由 app 的 vt100 画布覆盖,这里只画外壳
    pub fn remote_view(target: &mut FrameBuffer, working: bool) -> anyhow::Result<()> {
        let elems = vec![
            Elem::Title {
                text: "Remote".into(),
                center: false,
            },
            Elem::Icons {
                bt: false,
                wf: working,
            },
            Elem::Text {
                x: 0,
                y: SCREEN_H / 2 - 7,
                text: if working { "working" } else { "stop" }.into(),
                ink: if working { Ink::Ok } else { Ink::Idle },
                center: true,
            },
        ];
        render(&elems, target)
    }

    /// 通用状态页(OTA 等流程页):标题嵌片 + 居中多行正文。
    /// 典型:status(target, "OTA", "Connecting wifi", Ink::Text)
    pub fn status(
        target: &mut FrameBuffer,
        title: &str,
        body: &str,
        ink: Ink,
    ) -> anyhow::Result<()> {
        let (y0, step) = if cfg!(feature = "max2") {
            (44, 20)
        } else {
            (30, 16)
        };
        let mut elems = vec![Elem::Title {
            text: title.into(),
            center: false,
        }];
        for (i, line) in body.lines().enumerate() {
            elems.push(Elem::Text {
                x: 0,
                y: y0 + i as i32 * step,
                text: line.into(),
                ink,
                center: true,
            });
        }
        render(&elems, target)
    }

    /// SNTP 对时页:左上角嵌片显示 "Sync time"(借道 render_keyboard_view 时会错标成
    /// Keyboard,故单开一页)。
    pub fn sync_time_view(target: &mut FrameBuffer, msg: &str) -> anyhow::Result<()> {
        status(target, "Sync time", msg, Ink::Text)
    }

    /// keyboard 模式会话展示(旧 render_session_view):
    /// 整行底色表状态 → 行首标记表状态(perm ▲ / err × / active ● / idle ◆),
    /// 非 active(需关注)排前,与旧版同序。
    pub fn session_view(
        target: &mut FrameBuffer,
        wifi_on: bool,
        ble_on: bool,
        items: &[crate::sessions::SessionEntry],
    ) -> anyhow::Result<()> {
        let refs: Vec<&crate::sessions::SessionEntry> = items.iter().collect();
        let elems = super::kb_session_elems(wifi_on, ble_on, &refs, None);
        render(&elems, target)
    }

    /// keyboard 模式旋钮会话选择器(旧 render_knob_picker):同 session_view,
    /// 焦点行走焦点条(替代旧版的 "> <" 尖括号标记)。
    pub fn knob_picker(
        target: &mut FrameBuffer,
        wifi_on: bool,
        ble_on: bool,
        items: &[&crate::sessions::SessionEntry],
        focus: usize,
    ) -> anyhow::Result<()> {
        let elems = super::kb_session_elems(wifi_on, ble_on, items, Some(focus));
        render(&elems, target)
    }

    /// ASR 结果编辑页:标题嵌片 + 多行正文(含 ANSI 光标高亮)+ 键位提示。
    /// body 由调用方传 cursor_text()(光标字符包 \x1b[44m..[49m)。
    pub fn asr_editor(target: &mut FrameBuffer, body: &str) -> anyhow::Result<()> {
        let elems = vec![
            Elem::Title {
                text: "ASR".into(),
                center: false,
            },
            Elem::Para {
                x: FRAME_X + 10,
                y: 28,
                w: FRAME_W - 20,
                text: body.into(),
                ink: Ink::Text,
            },
            Elem::Footer {
                left: "Accept=Send  Esc=Cancel".into(),
                right: "Wheel=Move".into(),
            },
        ];
        render(&elems, target)
    }
}

/// keyboard 会话屏公共构建:标题 + 状态图标 + 状态行(可选焦点行 + 滚动窗口)
fn kb_session_elems(
    wifi_on: bool,
    ble_on: bool,
    items: &[&crate::sessions::SessionEntry],
    focus: Option<usize>,
) -> Vec<Elem> {
    let list_top = LIST_TOP;
    let bottom = FRAME_Y + FRAME_H;
    let visible = (((bottom - list_top) / ROW_STEP) as usize).max(1);
    let mut elems = vec![
        Elem::Title {
            text: "Sessions".into(),
            center: false,
        },
        Elem::Icons {
            bt: ble_on,
            wf: wifi_on,
        },
    ];
    if items.is_empty() {
        elems.push(Elem::Text {
            x: FRAME_X + 12,
            y: list_top + 2,
            text: "No sessions".into(),
            ink: Ink::Dim,
            center: false,
        });
        return elems;
    }
    // 稳定分区:非 active 优先,组内保持 upsert 先后(与旧版一致)。
    let (attention, active): (
        Vec<&crate::sessions::SessionEntry>,
        Vec<&crate::sessions::SessionEntry>,
    ) = items.iter().partition(|e| !e.st.is_active());
    let start = focus.map(|f| f.saturating_sub(visible.saturating_sub(1)));
    for (i, e) in attention
        .iter()
        .chain(active.iter())
        .enumerate()
        .skip(start.unwrap_or(0))
        .take(visible)
    {
        let y = list_top + (i - start.unwrap_or(0)) as i32 * ROW_STEP;
        let status = if e.st.is_perm() {
            Status::Perm
        } else if e.st.is_active() {
            Status::Work
        } else if e.st == crate::sessions::SessionStatus::Err {
            Status::Err
        } else {
            Status::Idle
        };
        let is_focus = focus == Some(i);
        if is_focus {
            elems.push(Elem::FocusBar { y });
        }
        elems.push(Elem::Row {
            y,
            row: Row::new(format!("{} ({})", e.proj, e.sid), status),
            focus: is_focus,
        });
    }
    elems
}

/// SNTP 对时页(小屏回退 display_text;main.rs 的调用点不带 cfg,故放 compat 外)
pub fn sync_time_view(target: &mut FrameBuffer, msg: &str) -> anyhow::Result<()> {
    compat::status(target, "Sync time", msg, Ink::Text)
}

/// 弹窗面板(ui.rs 的增量 Popup 用):PANEL 底 + 语义色描边 + 4 角 sprite + 居中文本。
/// slot_idx = 描边色的调色板槽(ACCENT/OK/PERM/ERR)。只画不 flush,调用方负责 flush_rect。
pub fn draw_popup_panel(
    target: &mut FrameBuffer,
    rect: Rectangle,
    text: &str,
    slot_idx: usize,
) -> anyhow::Result<()> {
    fill(target, rect, pal(slot::PANEL))?;
    stroke(target, rect, pal(slot_idx))?;
    let cs = 6;
    let (w, h) = (rect.size.width as i32, rect.size.height as i32);
    if w >= cs * 2 && h >= cs * 2 {
        let (x, y) = (rect.top_left.x, rect.top_left.y);
        for (s, cx, cy) in [
            (atlas::sprite::BOX_TL, x, y),
            (atlas::sprite::BOX_TR, x + w - cs, y),
            (atlas::sprite::BOX_BL, x, y + h - cs),
            (atlas::sprite::BOX_BR, x + w - cs, y + h - cs),
        ] {
            let (sx, sy, sw, sh) = s;
            atlas::blit(target, sx, sy, sw, sh, cx, cy)?;
        }
    }
    // 文字内缩 2px 留出描边;TextBox 支持多行(\n)
    draw_str(
        target,
        text,
        Rectangle::new(
            Point::new(rect.top_left.x + 2, rect.top_left.y + 2),
            Size::new(w.saturating_sub(4) as u32, h.saturating_sub(4) as u32),
        ),
        pal(slot::TEXT),
        HorizontalAlignment::Center,
    )?;
    Ok(())
}

/// 网络流程页(WiFi / MQTT / Tailscale 连接进度):左上角嵌片固定 "Network"
pub fn net_view(target: &mut FrameBuffer, msg: &str) -> anyhow::Result<()> {
    compat::status(target, "Network", msg, Ink::Text)
}

/// 同 net_view,正文错误色
pub fn net_view_err(target: &mut FrameBuffer, msg: &str) -> anyhow::Result<()> {
    compat::status(target, "Network", msg, Ink::Err)
}

/* ============ 底层原语(FrameBuffer 直写;颜色一律来自 pal()) ============ */

fn fill(target: &mut FrameBuffer, rect: Rectangle, color: ColorFormat) -> anyhow::Result<()> {
    Ok(rect.draw_styled(&PrimitiveStyle::with_fill(color), target)?)
}

/// 1px 描边(线宽写死;边线不进 atlas,花样只从角贴取)
fn stroke(target: &mut FrameBuffer, rect: Rectangle, color: ColorFormat) -> anyhow::Result<()> {
    Ok(rect.draw_styled(&PrimitiveStyle::with_stroke(color, 1), target)?)
}

/// 画一行文本:纯 ASCII 用 FONT_7X13_BOLD,含中文/全角符号时用 wqy12(缺字自动跳过)
fn draw_str(
    target: &mut FrameBuffer,
    s: &str,
    rect: Rectangle,
    color: ColorFormat,
    align: HorizontalAlignment,
) -> anyhow::Result<()> {
    let style = TextBoxStyleBuilder::new()
        .alignment(align)
        .height_mode(HeightMode::ShrinkToText(VerticalOverdraw::FullRowsOnly))
        .line_height(LineHeight::Pixels(LINE_H))
        .build();
    if s.is_ascii() {
        TextBox::with_textbox_style(s, rect, MonoTextStyle::new(&FONT_7X13_BOLD, color), style)
            .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
            .draw(target)?;
    } else {
        // MyTextStyle 的 vertical_offset 修 wqy12 中文上偏 3px(见 lcd.rs)
        TextBox::with_textbox_style(
            s,
            rect,
            crate::lcd::MyTextStyle {
                font_style: U8g2TextStyle::new(u8g2_font_wqy12_t_gb2312, color),
                vertical_offset: 3,
                bg_color: None,
            },
            style,
        )
        .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
        .draw(target)?;
    }
    Ok(())
}

/// 文本宽估算(ASCII 7px / 全角 13px——wqy12 实测 13px,按 12 算会低估导致嵌片挖坑过窄)
pub fn text_w(s: &str) -> i32 {
    s.chars().map(|c| if c.is_ascii() { 7 } else { 13 }).sum()
}

/* ============ 交互页面(与 ui.rs 同签名的接入口) ============ */

type Btn<'a> = &'a mut crate::AnyBtn;

#[derive(Debug)]
enum MenuEvt {
    Next,
    Accept,
    Esc,
}

/// 开机主菜单(ui2 版):渲染走元素表,按键语义与 ui::boot_menu 一致 —
/// Next 正向切换、Accept 进入、Esc 逆向。小屏(非 max2)布局不放,直接回退 ui.rs 版。
pub async fn boot_menu(
    target: &mut FrameBuffer,
    accept: Btn<'_>,
    esc: Btn<'_>,
    next: Btn<'_>,
) -> crate::ui::BootChoice {
    const CHOICES: [crate::ui::BootChoice; 3] = [
        crate::ui::BootChoice::Keyboard,
        crate::ui::BootChoice::Remote,
        crate::ui::BootChoice::Setting,
    ];
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let mut focus: usize = 0;

    loop {
        let elems = boot(version, focus);
        let _ = render(&elems, target);

        let evt = tokio::select! {
            _ = next.wait_for_falling_edge() => MenuEvt::Next,
            _ = accept.wait_for_falling_edge() => MenuEvt::Accept,
            _ = esc.wait_for_falling_edge() => MenuEvt::Esc,
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await; // 消抖

        match evt {
            MenuEvt::Next if next.is_low() => focus = (focus + 1) % CHOICES.len(),
            MenuEvt::Accept if accept.is_low() => return CHOICES[focus],
            MenuEvt::Esc if esc.is_low() => focus = (focus + CHOICES.len() - 1) % CHOICES.len(),
            _ => {}
        }
    }
}

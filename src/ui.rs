//! 手写 UI:开机菜单 / Setting / 各模式外壳。
//!
//! 基于 embedded-graphics + u8g2 中文字体,直接画到 `lcd::FrameBuffer`。
//! vibekeys 无触屏,导航用旋钮(pin16/17)上下移动焦点、Accept(btn7)确认。

use embedded_graphics::{
    image::GetPixel,
    mono_font::{ascii::FONT_7X13_BOLD, MonoTextStyle},
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, StyledDrawable},
    text::{Alignment, Baseline, LineHeight, Text, TextStyleBuilder},
};
use embedded_text::{
    alignment::HorizontalAlignment,
    style::{HeightMode, TextBoxStyleBuilder, VerticalOverdraw},
    TextBox,
};
use u8g2_fonts::{fonts::u8g2_font_open_iconic_all_2x_t, U8g2TextStyle};

use crate::lcd::{ColorFormat, DisplayTargetDrive, FrameBuffer};

type Btn<'a> = &'a mut crate::AnyBtn;

const LINE_H: u32 = 14;

// ========== 绘制工具 ==========

fn clear(target: &mut FrameBuffer, color: ColorFormat) -> anyhow::Result<()> {
    target.fill_color(color)
}

fn fill_rect(target: &mut FrameBuffer, rect: Rectangle, color: ColorFormat) -> anyhow::Result<()> {
    Ok(rect.draw_styled(&PrimitiveStyle::with_fill(color), target)?)
}

/// 在 `rect` 内画文本(支持中英)。`bg=Some` 时给文本填背景(用于焦点高亮)。
fn draw_text(
    target: &mut FrameBuffer,
    text: &str,
    rect: Rectangle,
    color: ColorFormat,
    bg: Option<ColorFormat>,
    align: HorizontalAlignment,
) -> anyhow::Result<()> {
    if let Some(bg) = bg {
        fill_rect(target, rect, bg)?;
    }
    let style = TextBoxStyleBuilder::new()
        .alignment(align)
        .height_mode(HeightMode::ShrinkToText(VerticalOverdraw::FullRowsOnly))
        .line_height(LineHeight::Pixels(LINE_H))
        .build();
    TextBox::with_textbox_style(
        text,
        rect,
        MonoTextStyle::new(&FONT_7X13_BOLD, color),
        style,
    )
    .draw(target)?;
    Ok(())
}

/// 画一个 u8g2 open_iconic 图标(按坐标,不裁剪)。
fn draw_icon(
    target: &mut FrameBuffer,
    icon: char,
    point: Point,
    color: ColorFormat,
) -> anyhow::Result<()> {
    Text::with_text_style(
        &icon.to_string(),
        point,
        U8g2TextStyle::new(u8g2_font_open_iconic_all_2x_t, color),
        TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Top)
            .build(),
    )
    .draw(target)?;
    Ok(())
}

fn flush(target: &mut FrameBuffer) -> anyhow::Result<()> {
    target.flush()
}

// ========== 开机菜单 ==========

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum BootChoice {
    Keyboard,
    Remote,
    Setting,
}

const BOOT_LABELS: [&str; 3] = ["Keyboard", "Remote", "Setting"];
const BOOT_CHOICES: [BootChoice; 3] = [
    BootChoice::Keyboard,
    BootChoice::Remote,
    BootChoice::Setting,
];

enum MenuEvt {
    Rotate,
    Accept,
    Esc,
}

/// 开机主菜单:旋钮上下选,Accept 进入。返回选中的模式。
pub async fn boot_menu(
    target: &mut FrameBuffer,
    accept: Btn<'_>,
    esc: Btn<'_>,
    rot_a: Btn<'_>,
    rot_b: Btn<'_>,
) -> BootChoice {
    let mut focus: usize = 0;
    let width = target.bounding_box().size.width;

    loop {
        let _ = render_boot_menu(target, focus, width);

        // select 只负责等事件;读旋钮电平放到 select 之外,避免 future 与 &mut 借用冲突。
        let evt = tokio::select! {
            _ = rot_a.wait_for_any_edge() => MenuEvt::Rotate,
            _ = accept.wait_for_low() => MenuEvt::Accept,
            _ = esc.wait_for_low() => MenuEvt::Esc,
        };

        match evt {
            MenuEvt::Rotate => {
                let down = rot_a.is_high() == rot_b.is_low();
                let n = BOOT_LABELS.len();
                focus = if down {
                    (focus + 1) % n
                } else {
                    (focus + n - 1) % n
                };
            }
            MenuEvt::Accept => return BOOT_CHOICES[focus],
            MenuEvt::Esc => {
                let n = BOOT_LABELS.len();
                focus = (focus + n - 1) % n;
            }
        }
    }
}

fn render_boot_menu(target: &mut FrameBuffer, focus: usize, width: u32) -> anyhow::Result<()> {
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        "VibeKeys",
        Rectangle::new(Point::new(4, 0), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHEAT,
        None,
        HorizontalAlignment::Center,
    )?;

    let item_h = LINE_H + 4;
    let start_y: i32 = 24;
    for (i, label) in BOOT_LABELS.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(0, start_y + (i as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        if i == focus {
            fill_rect(target, rect, ColorFormat::CSS_DARK_CYAN)?;
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHITE,
                Some(ColorFormat::CSS_DARK_CYAN),
                HorizontalAlignment::Center,
            )?;
        } else {
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHEAT,
                None,
                HorizontalAlignment::Center,
            )?;
        }
    }
    flush(target)
}

// ========== Setting 页面 ==========

/// 密码字符轮:0-9 a-z A-Z。
const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[derive(Copy, Clone, Eq, PartialEq)]
enum SettingState {
    Menu,
    WifiList,
    Password,
}

#[derive(Copy, Clone)]
enum InputEvt {
    Rotate,
    Accept,
    Esc,
}

pub enum SettingOutcome {
    Back,
    Ota,
}

/// 等一个输入事件。旋钮方向在返回后用 rot_a/rot_b 电平判断。
async fn wait_input(rot_a: Btn<'_>, accept: Btn<'_>, esc: Btn<'_>) -> InputEvt {
    tokio::select! {
        _ = rot_a.wait_for_any_edge() => InputEvt::Rotate,
        _ = accept.wait_for_low() => InputEvt::Accept,
        _ = esc.wait_for_low() => InputEvt::Esc,
    }
}

fn rotate_index(focus: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        0
    } else if down {
        (focus + 1) % len
    } else {
        (focus + len - 1) % len
    }
}

fn rot_down(rot_a: &crate::AnyBtn, rot_b: &crate::AnyBtn) -> bool {
    rot_a.is_high() == rot_b.is_low()
}

pub async fn setting_page(
    target: &mut FrameBuffer,
    wifi: &mut esp_idf_svc::wifi::EspWifi<'static>,
    sysloop: esp_idf_svc::eventloop::EspSystemEventLoop,
    accept: Btn<'_>,
    esc: Btn<'_>,
    rot_a: Btn<'_>,
    rot_b: Btn<'_>,
    setting: &mut crate::bt_wifi_mode::Setting,
    nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
) -> SettingOutcome {
    let mut state = SettingState::Menu;
    let mut menu_focus: usize = 0;
    let mut wifi_focus: usize = 0;
    let mut ssids: Vec<String> = Vec::new();
    let mut password: String = setting.pass.clone();
    let mut cur_char: usize = 0;

    loop {
        match state {
            SettingState::Menu => {
                let _ = render_setting_menu(target, menu_focus, setting, &password);
                match wait_input(rot_a, accept, esc).await {
                    InputEvt::Rotate => {
                        menu_focus = rotate_index(menu_focus, 3, rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => match menu_focus {
                        0 => {
                            // 扫描 WiFi
                            let _ = clear(target, ColorFormat::CSS_BLACK);
                            let _ = draw_text(
                                target,
                                "Scanning WiFi...",
                                target.bounding_box(),
                                ColorFormat::CSS_WHEAT,
                                None,
                                HorizontalAlignment::Center,
                            );
                            let _ = flush(target);
                            ssids = crate::wifi::scan(wifi, sysloop.clone()).unwrap_or_default();
                            if ssids.is_empty() {
                                ssids.push("(none)".to_string());
                            }
                            wifi_focus = 0;
                            state = SettingState::WifiList;
                        }
                        1 => {
                            cur_char = 0;
                            state = SettingState::Password;
                        }
                        2 => return SettingOutcome::Ota,
                        _ => {}
                    },
                    InputEvt::Esc => {
                        save_wifi_config(setting, &password, nvs);
                        return SettingOutcome::Back;
                    }
                }
            }
            SettingState::WifiList => {
                let _ = render_list(target, "WiFi (ESC=back)", &ssids, wifi_focus);
                match wait_input(rot_a, accept, esc).await {
                    InputEvt::Rotate => {
                        wifi_focus = rotate_index(wifi_focus, ssids.len(), rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => {
                        let picked = ssids[wifi_focus].clone();
                        if picked != "(none)" {
                            setting.ssid = picked;
                            let _ = nvs.set_str("ssid", &setting.ssid);
                        }
                        state = SettingState::Menu;
                    }
                    InputEvt::Esc => state = SettingState::Menu,
                }
            }
            SettingState::Password => {
                let _ = render_password(target, &password, cur_char);
                match wait_input(rot_a, accept, esc).await {
                    InputEvt::Rotate => {
                        cur_char = rotate_index(cur_char, CHARSET.len(), rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => {
                        if password.len() < 32 {
                            password.push(CHARSET[cur_char] as char);
                        }
                    }
                    InputEvt::Esc => {
                        setting.pass = password.clone();
                        let _ = nvs.set_str("pass", &setting.pass);
                        state = SettingState::Menu;
                    }
                }
            }
        }
    }
}

fn save_wifi_config(
    setting: &mut crate::bt_wifi_mode::Setting,
    password: &str,
    nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
) {
    setting.pass = password.to_string();
    let _ = nvs.set_str("pass", &setting.pass);
    let _ = nvs.set_str("ssid", &setting.ssid);
}

fn render_setting_menu(
    target: &mut FrameBuffer,
    focus: usize,
    setting: &crate::bt_wifi_mode::Setting,
    password: &str,
) -> anyhow::Result<()> {
    let width = target.bounding_box().size.width;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        "Setting",
        Rectangle::new(Point::new(4, 0), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHEAT,
        None,
        HorizontalAlignment::Center,
    )?;
    let pass_label = if password.is_empty() {
        "(none)".to_string()
    } else {
        "*".repeat(password.len())
    };
    let items = [
        format!("WiFi: {}", setting.ssid),
        format!("Pass: {}", pass_label),
        "OTA Update".to_string(),
    ];
    let item_h = LINE_H + 4;
    let start_y: i32 = 24;
    for (i, label) in items.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(0, start_y + (i as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        if i == focus {
            fill_rect(target, rect, ColorFormat::CSS_DARK_CYAN)?;
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHITE,
                Some(ColorFormat::CSS_DARK_CYAN),
                HorizontalAlignment::Left,
            )?;
        } else {
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHEAT,
                None,
                HorizontalAlignment::Left,
            )?;
        }
    }
    flush(target)
}

fn render_list(
    target: &mut FrameBuffer,
    title: &str,
    items: &[String],
    focus: usize,
) -> anyhow::Result<()> {
    let width = target.bounding_box().size.width;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        title,
        Rectangle::new(Point::new(4, 0), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHEAT,
        None,
        HorizontalAlignment::Left,
    )?;
    let item_h = LINE_H + 2;
    let start_y: i32 = 18;
    for (i, label) in items.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(0, start_y + (i as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        if i == focus {
            fill_rect(target, rect, ColorFormat::CSS_DARK_CYAN)?;
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHITE,
                Some(ColorFormat::CSS_DARK_CYAN),
                HorizontalAlignment::Left,
            )?;
        } else {
            draw_text(
                target,
                label,
                rect,
                ColorFormat::CSS_WHEAT,
                None,
                HorizontalAlignment::Left,
            )?;
        }
    }
    flush(target)
}

fn render_password(target: &mut FrameBuffer, password: &str, focus: usize) -> anyhow::Result<()> {
    let width = target.bounding_box().size.width;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        "Password BkSp=done",
        Rectangle::new(Point::new(4, 0), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHEAT,
        None,
        HorizontalAlignment::Left,
    )?;
    draw_text(
        target,
        password,
        Rectangle::new(Point::new(4, 18), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHITE,
        None,
        HorizontalAlignment::Left,
    )?;
    // 字符轮盘:一排字符,中间高亮(= focus),旋钮/左右键滑动
    let n = ((width / 16) as usize).clamp(5, 11);
    let cell_w = width / n as u32;
    let half = n / 2;
    let cell_h = LINE_H + 6;
    let y = 38;
    for k in 0..n {
        let idx = (focus + k + CHARSET.len() - half) % CHARSET.len();
        let x = (k as u32) * cell_w;
        let rect = Rectangle::new(Point::new(x as i32, y), Size::new(cell_w, cell_h));
        let ch = (CHARSET[idx] as char).to_string();
        if idx == focus {
            fill_rect(target, rect, ColorFormat::CSS_DARK_CYAN)?;
            draw_text(
                target,
                &ch,
                rect,
                ColorFormat::CSS_WHITE,
                Some(ColorFormat::CSS_DARK_CYAN),
                HorizontalAlignment::Center,
            )?;
        } else {
            draw_text(
                target,
                &ch,
                rect,
                ColorFormat::CSS_WHEAT,
                None,
                HorizontalAlignment::Center,
            )?;
        }
    }
    flush(target)
}

// ========== 模式外壳(键盘 / Remote) ==========

const STATUS_H: u32 = 16;

/// 顶部状态栏:蓝牙 / WiFi 连接状态。
pub fn draw_status_bar(
    target: &mut FrameBuffer,
    wifi_on: bool,
    ble_on: Option<bool>,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let bar = Rectangle::new(Point::new(0, 0), Size::new(bb.size.width, STATUS_H));
    fill_rect(target, bar, ColorFormat::CSS_DARK_SLATE_GRAY)?;
    let icon_w: u32 = 18;
    let gap: u32 = 6;
    let mut x: u32 = 2;
    if let Some(ble) = ble_on {
        if ble {
            // 激活:蓝底
            fill_rect(
                target,
                Rectangle::new(Point::new(x as i32, 0), Size::new(icon_w, STATUS_H)),
                ColorFormat::CSS_BLUE,
            )?;
        }
        draw_icon(
            target,
            '\u{5E}',
            Point::new(x as i32 + icon_w as i32 / 2, 0),
            if ble {
                ColorFormat::CSS_WHITE
            } else {
                ColorFormat::CSS_GRAY
            },
        )?;
        x += icon_w + gap;
    }
    draw_icon(
        target,
        '\u{F8}',
        Point::new(x as i32 + icon_w as i32 / 2, 0),
        if wifi_on {
            ColorFormat::CSS_WHITE
        } else {
            ColorFormat::CSS_GRAY
        },
    )?;
    Ok(())
}

/// 键盘模式视图:状态栏 + 动画区(stop/working 占位)+ 反馈文字。
pub fn render_keyboard_view(
    target: &mut FrameBuffer,
    wifi_on: bool,
    ble_on: bool,
    feedback: &str,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_status_bar(target, wifi_on, Some(ble_on))?;
    let anim = Rectangle::new(
        Point::new(0, STATUS_H as i32),
        Size::new(bb.size.width, bb.size.height.saturating_sub(STATUS_H)),
    );
    let bg = if ble_on {
        ColorFormat::CSS_DARK_GREEN
    } else {
        ColorFormat::CSS_DARK_RED
    };
    fill_rect(target, anim, bg)?;
    draw_text(
        target,
        if ble_on { "working" } else { "stop" },
        Rectangle::new(
            Point::new(0, STATUS_H as i32),
            Size::new(bb.size.width, LINE_H + 2),
        ),
        ColorFormat::CSS_WHITE,
        None,
        HorizontalAlignment::Center,
    )?;
    if !feedback.is_empty() {
        draw_text(
            target,
            feedback,
            anim,
            ColorFormat::CSS_WHITE,
            None,
            HorizontalAlignment::Center,
        )?;
    }
    flush(target)
}

/// Remote 模式视图:stop 显示占位提示,working 显示动画占位。
/// (working 时实际屏幕由 app::run 的 ui.handle_message 显示 vibetty 实时画面覆盖。)
pub fn render_remote_view(target: &mut FrameBuffer, working: bool) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_status_bar(target, working, None)?;
    let anim = Rectangle::new(
        Point::new(0, STATUS_H as i32),
        Size::new(bb.size.width, bb.size.height.saturating_sub(STATUS_H)),
    );
    let (bg, label) = if working {
        (ColorFormat::CSS_DARK_GREEN, "Remote: working")
    } else {
        (ColorFormat::CSS_DARK_BLUE, "Remote: stop")
    };
    fill_rect(target, anim, bg)?;
    draw_text(
        target,
        label,
        anim,
        ColorFormat::CSS_WHITE,
        None,
        HorizontalAlignment::Center,
    )?;
    flush(target)
}

// ========== 中央弹窗(panel 增量重绘) ==========

/// 中央弹窗:TUI 风格描边。`show` 时 backup 弹窗区域、画描边+文字、`flush_rect`
/// 只推弹窗区域;`hide` 时 restore backup。屏幕其他部分不动(增量重绘)。
pub struct Popup {
    rect: Rectangle,
    backup: Option<Vec<ColorFormat>>,
}

impl Popup {
    /// 居中弹窗(屏幕 80% 宽 × 36 高)。
    pub fn new_centered(bb: Rectangle) -> Self {
        let w = bb.size.width * 4 / 5;
        let h = 36u32;
        let x = bb.top_left.x + ((bb.size.width - w) / 2) as i32;
        let y = bb.top_left.y + ((bb.size.height - h) / 2) as i32;
        Self {
            rect: Rectangle::new(Point::new(x, y), Size::new(w, h)),
            backup: None,
        }
    }

    /// 显示弹窗。若已打开则只重画内容(不重复 backup)。
    pub fn show(&mut self, target: &mut FrameBuffer, text: &str) -> anyhow::Result<()> {
        if self.backup.is_none() {
            self.backup = Some(backup_rect(target, self.rect));
        }
        draw_popup(target, self.rect, text)?;
        target.flush_rect(self.rect)?;
        Ok(())
    }

    /// 关闭弹窗并恢复原画面。
    pub fn hide(&mut self, target: &mut FrameBuffer) -> anyhow::Result<()> {
        if let Some(b) = self.backup.take() {
            restore_rect(target, self.rect, &b)?;
            target.flush_rect(self.rect)?;
        }
        Ok(())
    }
}

/// 便捷构造:按屏幕 bounding_box 居中弹窗。
pub fn popup_centered(target: &FrameBuffer) -> Popup {
    Popup::new_centered(target.bounding_box())
}

fn backup_rect(target: &FrameBuffer, rect: Rectangle) -> Vec<ColorFormat> {
    let r = rect.intersection(&target.bounding_box());
    let mut v = Vec::with_capacity((r.size.width * r.size.height) as usize);
    for y in 0..r.size.height as i32 {
        for x in 0..r.size.width as i32 {
            let p = Point::new(r.top_left.x + x, r.top_left.y + y);
            v.push(target.pixel(p).unwrap_or(ColorFormat::CSS_BLACK));
        }
    }
    v
}

fn restore_rect(
    target: &mut FrameBuffer,
    rect: Rectangle,
    backup: &[ColorFormat],
) -> anyhow::Result<()> {
    let r = rect.intersection(&target.bounding_box());
    let w = r.size.width as usize;
    let pixels = (0..r.size.height as i32).flat_map(|y| {
        (0..r.size.width as i32).map(move |x| {
            let idx = (y as usize) * w + (x as usize);
            Pixel(Point::new(r.top_left.x + x, r.top_left.y + y), backup[idx])
        })
    });
    target.draw_iter(pixels)?;
    Ok(())
}

fn draw_popup(target: &mut FrameBuffer, rect: Rectangle, text: &str) -> anyhow::Result<()> {
    let r = rect.intersection(&target.bounding_box());
    fill_rect(target, r, ColorFormat::CSS_BLACK)?;
    // TUI 风格描边
    r.draw_styled(
        &PrimitiveStyle::with_stroke(ColorFormat::CSS_WHITE, 1),
        target,
    )?;
    // 文字内缩 2px,留出描边
    let inner = Rectangle::new(
        Point::new(r.top_left.x + 2, r.top_left.y + 2),
        Size::new(
            r.size.width.saturating_sub(4),
            r.size.height.saturating_sub(4),
        ),
    );
    draw_text(
        target,
        text,
        inner,
        ColorFormat::CSS_WHITE,
        Some(ColorFormat::CSS_BLACK),
        HorizontalAlignment::Center,
    )?;
    Ok(())
}

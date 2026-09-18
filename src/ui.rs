//! 手写 UI:开机菜单 / Setting / 各模式外壳。
//!
//! 基于 embedded-graphics + u8g2 中文字体,直接画到 `lcd::FrameBuffer`。
//! vibekeys 无触屏,菜单用 Next(btn4)切换选项、Accept(btn7)确认;子列表(WiFi/密码字符)
//! 仍可用旋钮(pin16/17)双向滚动。

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
use u8g2_fonts::{
    fonts::{u8g2_font_open_iconic_all_1x_t, u8g2_font_wqy12_t_gb2312},
    U8g2TextStyle,
};

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

/// 在 `rect` 内画文本(**仅 ASCII**,菜单/标签用)。`bg=Some` 时给文本填背景(用于焦点高亮)。
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

/// 与 `draw_text` 同形,但用 u8g2 文泉驿字体(**支持中文**),给 ASR 等可能含中文的文本用。
/// 字体缺字时 U8g2TextStyle 默认跳过(`ignore_unknown_chars`),不会乱码。
fn draw_text_cjk(
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
        // 用 lcd::MyTextStyle 而非裸 U8g2TextStyle:u8g2 的文泉驿中文会向上偏 3px,
        // MyTextStyle 的 draw_string 里 position.y += vertical_offset 把它修回来。
        crate::lcd::MyTextStyle {
            font_style: U8g2TextStyle::new(u8g2_font_wqy12_t_gb2312, color),
            vertical_offset: 3,
            bg_color: None,
        },
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
        U8g2TextStyle::new(u8g2_font_open_iconic_all_1x_t, color),
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

/// 显示一帧 JPEG 屏幕帧(直接刷 LCD,等价 `lcd::display_jpeg`)。
/// 放在 ui.rs 便于 app.rs 与 popup 等统一从 `ui::` 调用。
pub fn display_jpeg(jpeg: &[u8]) -> anyhow::Result<()> {
    crate::lcd::display_jpeg(jpeg)
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

#[derive(Debug)]
enum MenuEvt {
    Next,
    Accept,
    Esc,
}

/// 开机主菜单:Next 键正向切换选项、Accept 进入、Esc 逆向。返回选中的模式。
pub async fn boot_menu(
    target: &mut FrameBuffer,
    accept: Btn<'_>,
    esc: Btn<'_>,
    next: Btn<'_>,
) -> BootChoice {
    let mut focus: usize = 0;
    let width = target.bounding_box().size.width;
    let n = BOOT_LABELS.len();

    loop {
        let _ = render_boot_menu(target, focus, width);

        let evt = tokio::select! {
            _ = next.wait_for_falling_edge() => MenuEvt::Next,
            _ = accept.wait_for_falling_edge() => MenuEvt::Accept,
            _ = esc.wait_for_falling_edge() => MenuEvt::Esc,
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await; // 消抖

        match evt {
            MenuEvt::Next if next.is_low() => focus = (focus + 1) % n,
            MenuEvt::Accept if accept.is_low() => return BOOT_CHOICES[focus],
            MenuEvt::Esc if esc.is_low() => focus = (focus + n - 1) % n,
            _ => {}
        }
    }
}

fn render_boot_menu(target: &mut FrameBuffer, focus: usize, width: u32) -> anyhow::Result<()> {
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        &format!("VibeKeys v{}", env!("CARGO_PKG_VERSION")),
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
    /// 已配置的 wifi_list(+ 尾部 <Add>)。
    WifiCreds,
    /// 从扫描结果里挑一个 ssid(用于新增)。
    ScanPicker,
    /// 字符轮编辑某条 cred 的密码。
    PassEditor,
}

#[derive(Copy, Clone)]
enum InputEvt {
    Rotate,
    Accept,
    Esc,
    Next,
    Backspace,
}

pub enum SettingOutcome {
    Back,
    Ota,
    ClearConfig,
    /// 硬件自检:显示按键按下/放开,MIC 按住时可视化录音电平。
    Test,
}

/// 等一个输入事件。旋钮方向在返回后用 rot_a/rot_b 电平判断。
/// Next(btn4)是主菜单切换选项的主力;旋钮在子列表(WiFi/密码字符)里仍可滚动。
/// Backspace(btn5)目前只在密码输入态用于删除光标前的字符。
async fn wait_input(
    rot_a: Btn<'_>,
    accept: Btn<'_>,
    esc: Btn<'_>,
    next: Btn<'_>,
    backspace: Btn<'_>,
) -> InputEvt {
    loop {
        let evt = tokio::select! {
            _ = rot_a.wait_for_any_edge() => InputEvt::Rotate,
            _ = accept.wait_for_falling_edge() => InputEvt::Accept,
            _ = esc.wait_for_falling_edge() => InputEvt::Esc,
            _ = next.wait_for_falling_edge() => InputEvt::Next,
            _ = backspace.wait_for_falling_edge() => InputEvt::Backspace,
        };
        // 旋钮(编码器):边沿即有效事件,方向由调用方读 rot_a/rot_b 电平判定,不消抖、不等。
        if matches!(evt, InputEvt::Rotate) {
            return evt;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await; // 消抖
                                                                        // 消抖后再确认仍处于按下,否则视为抖动丢弃,重新等下一个事件(对应 boot_menu 的 `_ => {}`)。
        let pressed = match evt {
            InputEvt::Accept => accept.is_low(),
            InputEvt::Esc => esc.is_low(),
            InputEvt::Next => next.is_low(),
            InputEvt::Backspace => backspace.is_low(),
            InputEvt::Rotate => true,
        };
        if pressed {
            return evt;
        }
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
    scan_list: &[String],
    accept: Btn<'_>,
    esc: Btn<'_>,
    next: Btn<'_>,
    rot_a: Btn<'_>,
    rot_b: Btn<'_>,
    backspace: Btn<'_>,
    setting: &mut crate::bt_wifi_mode::Setting,
    nvs: &mut esp_idf_svc::nvs::EspDefaultNvs,
) -> SettingOutcome {
    use crate::bt_wifi_mode::{Setting as BtSetting, WifiCred, MAX_WIFI_CREDS};

    let mut state = SettingState::Menu;
    let mut menu_focus: usize = 0;
    let mut cred_focus: usize = 0; // WifiCreds 列表焦点(含尾部 <Add>)
    let mut scan_focus: usize = 0; // ScanPicker 焦点
                                   // None=新增,Some(i)=正在编辑 wifi_list[i]。
    let mut cur_editing: Option<usize> = None;
    let mut pending_ssid: String = String::new();
    let mut password: String = String::new();
    let mut cur_char: usize = 0;

    loop {
        match state {
            SettingState::Menu => {
                let _ = render_setting_menu(target, menu_focus, setting);
                match wait_input(rot_a, accept, esc, next, backspace).await {
                    // 主菜单改用 Next 键切换选项;滚轮在这里不再切换(避免误触/跳格)。
                    InputEvt::Next => menu_focus = rotate_index(menu_focus, 4, true),
                    InputEvt::Rotate => {}
                    InputEvt::Accept => match menu_focus {
                        0 => {
                            cred_focus =
                                cred_focus.min(cred_entry_count(setting).saturating_sub(1));
                            state = SettingState::WifiCreds;
                        }
                        1 => return SettingOutcome::Ota,
                        // 清空配置的实际动作(操作 nvs)交给 main,这里只回报意图。
                        2 => return SettingOutcome::ClearConfig,
                        3 => return SettingOutcome::Test,
                        _ => {}
                    },
                    // 每条 cred 的增删改都即时落盘,Esc 直接返回即可。
                    InputEvt::Esc => return SettingOutcome::Back,
                    InputEvt::Backspace => {}
                }
            }
            SettingState::WifiCreds => {
                let labels = cred_labels(setting);
                let count = labels.len();
                let _ = render_list(target, "WiFi (ESC=back BkSp=del)", &labels, cred_focus);
                match wait_input(rot_a, accept, esc, next, backspace).await {
                    InputEvt::Next => cred_focus = rotate_index(cred_focus, count, true),
                    InputEvt::Rotate => {
                        cred_focus = rotate_index(cred_focus, count, rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => {
                        if cred_focus >= setting.wifi_list.len() {
                            // <Add>:进扫描选择器挑一个 ssid。
                            scan_focus = 0;
                            state = SettingState::ScanPicker;
                        } else {
                            // 编辑已有:载入它的 ssid/pass 进 PassEditor。
                            cur_editing = Some(cred_focus);
                            pending_ssid = setting.wifi_list[cred_focus].ssid.clone();
                            password = setting.wifi_list[cred_focus].pass.clone();
                            cur_char = 0;
                            state = SettingState::PassEditor;
                        }
                    }
                    InputEvt::Backspace => {
                        // 删除当前 cred(对 <Add> 项无效)。
                        if cred_focus < setting.wifi_list.len() {
                            setting.wifi_list.remove(cred_focus);
                            if let Err(e) = BtSetting::save_wifi_list(nvs, &setting.wifi_list) {
                                log::error!("Failed to save wifi_list: {:?}", e);
                            }
                            if cred_focus > 0 {
                                cred_focus -= 1;
                            }
                        }
                    }
                    InputEvt::Esc => state = SettingState::Menu,
                }
            }
            SettingState::ScanPicker => {
                let count = scan_list.len();
                let _ = render_list(target, "Pick network (ESC=back)", scan_list, scan_focus);
                match wait_input(rot_a, accept, esc, next, backspace).await {
                    InputEvt::Next => scan_focus = rotate_index(scan_focus, count, true),
                    InputEvt::Rotate => {
                        scan_focus = rotate_index(scan_focus, count, rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => {
                        if count > 0 {
                            let picked = scan_list[scan_focus].clone();
                            // 已存在同 ssid 则改成编辑它,避免重复条目。
                            cur_editing = setting.wifi_list.iter().position(|c| c.ssid == picked);
                            pending_ssid = picked;
                            password = match cur_editing {
                                Some(i) => setting.wifi_list[i].pass.clone(),
                                None => String::new(),
                            };
                            cur_char = 0;
                            state = SettingState::PassEditor;
                        }
                    }
                    InputEvt::Esc | InputEvt::Backspace => state = SettingState::WifiCreds,
                }
            }
            SettingState::PassEditor => {
                let _ = render_password(target, &pending_ssid, &password, cur_char);
                match wait_input(rot_a, accept, esc, next, backspace).await {
                    InputEvt::Next => cur_char = rotate_index(cur_char, CHARSET.len(), true),
                    InputEvt::Rotate => {
                        cur_char = rotate_index(cur_char, CHARSET.len(), rot_down(rot_a, rot_b));
                    }
                    InputEvt::Accept => {
                        if password.len() < 32 {
                            password.push(CHARSET[cur_char] as char);
                        }
                    }
                    InputEvt::Backspace => {
                        // 删除光标(末尾插入点)前的一个字符。
                        password.pop();
                    }
                    InputEvt::Esc => {
                        // 提交:更新已有 / 新增一条。
                        match cur_editing {
                            Some(i) if i < setting.wifi_list.len() => {
                                setting.wifi_list[i].ssid = pending_ssid.clone();
                                setting.wifi_list[i].pass = password.clone();
                            }
                            _ if setting.wifi_list.len() < MAX_WIFI_CREDS => {
                                setting.wifi_list.push(WifiCred {
                                    ssid: pending_ssid.clone(),
                                    pass: password.clone(),
                                });
                            }
                            _ => {}
                        }
                        if let Err(e) = BtSetting::save_wifi_list(nvs, &setting.wifi_list) {
                            log::error!("Failed to save wifi_list: {:?}", e);
                        }
                        // 回到 cred 列表,焦点回到刚编辑/新增的那条。
                        cred_focus =
                            cur_editing.unwrap_or(setting.wifi_list.len().saturating_sub(1));
                        cur_editing = None;
                        pending_ssid.clear();
                        password.clear();
                        state = SettingState::WifiCreds;
                    }
                }
            }
        }
    }
}

/// WifiCreds 列表条目数(含尾部 <Add>,达到上限时没有 <Add>)。
fn cred_entry_count(setting: &crate::bt_wifi_mode::Setting) -> usize {
    let n = setting.wifi_list.len();
    if n < crate::bt_wifi_mode::MAX_WIFI_CREDS {
        n + 1
    } else {
        n
    }
}

/// WifiCreds 列表显示文本:各 cred 的 ssid + 尾部 <Add>(达到上限时无)。
fn cred_labels(setting: &crate::bt_wifi_mode::Setting) -> Vec<String> {
    let mut v: Vec<String> = setting.wifi_list.iter().map(|c| c.ssid.clone()).collect();
    if v.len() < crate::bt_wifi_mode::MAX_WIFI_CREDS {
        v.push("<Add>".to_string());
    }
    v
}

fn render_setting_menu(
    target: &mut FrameBuffer,
    focus: usize,
    setting: &crate::bt_wifi_mode::Setting,
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
    let items = [
        format!("WiFi networks ({})", setting.wifi_list.len()),
        "OTA Update".to_string(),
        "Clear config".to_string(),
        "Hardware test".to_string(),
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

/// 硬件自检页(Setting → Hardware test,ESC 退出)。
///
/// - 任意按键按下/放开:屏幕顶部显示事件(如 `ACCEPT down` / `MIC up`),滚轮显示 Rotate 事件;
/// - MIC 按住:调用 `mic_read` 录音(底层 I2S),每个采样块取第一个采样画一根竖条,
///   从左往右画一屏柱状波形(每个 I2S 块取第一个采样);松开 MIC 后一次性显示整段。
///
/// `mic_read` 为 None(设备无音频驱动)时 MIC 只当普通按键显示,不录音。
pub async fn hardware_test_page(
    target: &mut FrameBuffer,
    accept: Btn<'_>,
    esc: Btn<'_>,
    next: Btn<'_>,
    backspace: Btn<'_>,
    mic: Btn<'_>,
    custom: Btn<'_>,
    switch: Btn<'_>,
    rot_a: Btn<'_>,
    rot_b: Btn<'_>,
    rot_button: Btn<'_>,
    mut audio: Option<&mut crate::audio::Driver>,
) {
    let bb = target.bounding_box();
    let width = bb.size.width;
    let height = bb.size.height;

    // 波形区:标题(2 行事件)下面到底部。条宽 2px、间隔 1px(3px/列),条高 = 块内峰值 × 增益。
    let wave_top: i32 = (LINE_H * 2 + 8) as i32;
    let wave_h = (height as i32 - wave_top).max(20) as u32;
    let wave_mid = wave_top + (wave_h as i32) / 2; // 中线:静音时画 0
    let bar_w: i32 = 2;
    let col_w: i32 = 3;
    let max_cols = (width as i32 / col_w) as usize;
    // I2S 16kHz mono,一次读 512 samples(约 32ms),取块内 |peak| 画条(单点采样随机性大,峰值才稳定)。
    let mut sample_buf = [0i16; 512];
    // 增益:语音峰值常在满幅的 10%-30%,×4 才能占满半屏;饱和就削顶。
    const GAIN: i32 = 4;

    let mut hist: Vec<u8> = Vec::with_capacity(max_cols); // 每列归一化幅值 0..=100
    let mut last_evt = String::from("Test keys & mic\nhold ESC to reboot");

    let mut last_states: Vec<(&str, bool)> = vec![]; // (名, 是否按下) — 仅用于结构化重画
                                                     // 逐键命名(数组索引过不了 select! 的借用检查)。
    let (n_mic, name_mic) = (mic, "MIC");
    let (n_custom, name_custom) = (custom, "CUSTOM");
    let (n_esc, name_esc) = (esc, "ESC");
    let (n_next, name_next) = (next, "NEXT");
    let (n_bksp, name_bksp) = (backspace, "BkSP");
    let (n_switch, name_switch) = (switch, "SWITCH");
    let (n_accept, name_accept) = (accept, "ACCEPT");
    let (n_ra, name_ra) = (rot_a, "ROT-A");
    let (n_rb, name_rb) = (rot_b, "ROT-B");
    let (n_rbtn, name_rbtn) = (rot_button, "ROT-BTN");

    let draw = |target: &mut FrameBuffer, hist: &[u8], evt: &str| {
        let _ = clear(target, ColorFormat::CSS_BLACK);
        let (line1, line2) = evt.split_once('\n').unwrap_or((evt, ""));
        let _ = draw_text(
            target,
            line1,
            Rectangle::new(Point::new(2, 0), Size::new(width - 2, LINE_H + 2)),
            ColorFormat::CSS_WHITE,
            None,
            HorizontalAlignment::Left,
        );
        if !line2.is_empty() {
            let _ = draw_text(
                target,
                line2,
                Rectangle::new(
                    Point::new(2, LINE_H as i32 + 2),
                    Size::new(width - 2, LINE_H + 2),
                ),
                ColorFormat::CSS_WHITE,
                None,
                HorizontalAlignment::Left,
            );
        }
        // 中线
        let _ = fill_rect(
            target,
            Rectangle::new(Point::new(0, wave_mid), Size::new(width, 1)),
            ColorFormat::CSS_DARK_GRAY,
        );
        for (i, amp) in hist.iter().enumerate() {
            // 两侧对称条:总高 = amp/100 × wave_h
            let h = ((*amp as u32) * wave_h / 100).max(1);
            let x = (i as i32) * col_w;
            let _ = fill_rect(
                target,
                Rectangle::new(
                    Point::new(x, wave_mid - (h as i32) / 2),
                    Size::new(bar_w as u32, h as u32),
                ),
                ColorFormat::CSS_GREEN,
            );
        }
        let _ = flush(target);
    };

    draw(target, &hist, &last_evt);

    loop {
        // 1) 逐键等任意边沿(一个 select 覆盖全部 10 个输入)。
        let (idx, is_low) = tokio::select! {
            _ = n_mic.wait_for_any_edge() => (0usize, n_mic.is_low()),
            _ = n_custom.wait_for_any_edge() => (1, n_custom.is_low()),
            _ = n_esc.wait_for_any_edge() => (2, n_esc.is_low()),
            _ = n_next.wait_for_any_edge() => (3, n_next.is_low()),
            _ = n_bksp.wait_for_any_edge() => (4, n_bksp.is_low()),
            _ = n_switch.wait_for_any_edge() => (5, n_switch.is_low()),
            _ = n_accept.wait_for_any_edge() => (6, n_accept.is_low()),
            _ = n_ra.wait_for_any_edge() => (7, true),
            _ = n_rb.wait_for_any_edge() => (8, true),
            _ = n_rbtn.wait_for_any_edge() => (9, n_rbtn.is_low()),
        };
        // 消抖:等 20ms 再确认电平(旋钮除外,边沿即事件)。
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (name, pressed) = match idx {
            0 => (name_mic, n_mic.is_low()),
            1 => (name_custom, n_custom.is_low()),
            2 => (name_esc, n_esc.is_low()),
            3 => (name_next, n_next.is_low()),
            4 => (name_bksp, n_bksp.is_low()),
            5 => (name_switch, n_switch.is_low()),
            6 => (name_accept, n_accept.is_low()),
            7 => (name_ra, true),
            8 => (name_rb, true),
            _ => (name_rbtn, n_rbtn.is_low()),
        };
        let _ = is_low;

        // ESC 也是被测按键;长按 1.2s 直接重启(比返回主菜单干净:音频驱动等外设
        // 无需归还,重启后全部归零)。按下先立即画出来,否则长按检测期间屏幕停留在
        // 上一事件,看不到 ESC down。
        if idx == 2 {
            if pressed {
                draw(target, &hist, "ESC down\nhold 1.2s to reboot");
            }
            let mut held_ms = 0u32;
            while n_esc.is_low() && held_ms < 1200 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                held_ms += 50;
            }
            if held_ms >= 1200 {
                esp_idf_svc::hal::reset::restart();
            }
        }

        // 旋钮旋转:只记事件,不区别方向(A/B 两相都触发)。
        let evt_text = if (7..=8).contains(&idx) {
            format!("Rotate!\n{name} edge")
        } else {
            format!("{} {}", name, if pressed { "down" } else { "up" })
        };
        last_evt = evt_text;
        // 记录该键最新状态(重画用)。
        if let Some(st) = last_states.iter_mut().find(|(n, _)| *n == name) {
            st.1 = pressed;
        } else {
            last_states.push((name, pressed));
        }

        // 2) MIC 按下且驱动可用 → 按住期间静默录音(不画屏),松开后一次性画整段波形。
        if idx == 0 && pressed {
            if let Some(driver) = audio.as_mut() {
                // 每块(512 samples ≈ 32ms)取第一个采样,read 返回字节数转样本数。
                // 录满 max_cols 块(≈ 一屏宽 ≈ 10s)或松开 MIC 即停。
                let mut rec: Vec<u8> = Vec::with_capacity(max_cols);
                loop {
                    if n_mic.is_high() {
                        break;
                    }
                    let bytes = driver.read(unsafe {
                        std::slice::from_raw_parts_mut(
                            sample_buf.as_mut_ptr() as *mut u8,
                            sample_buf.len() * 2,
                        )
                    });
                    if let Ok(n) = bytes {
                        if n > 0 {
                            // 块内峰值归一化 + 增益:|peak|×GAIN / 32768 → 0..=100(饱和削顶)
                            let samples = &sample_buf[..n / 2];
                            let peak = samples
                                .iter()
                                .map(|s| (s.abs() as i64) * (GAIN as i64))
                                .max()
                                .unwrap_or(0);
                            let amp = (peak.min(i16::MAX as i64) * 100 / 32768) as u8;
                            if rec.len() >= max_cols {
                                break; // 录满一屏,提前结束
                            }
                            rec.push(amp);
                        }
                    }
                }
                hist = rec;
                last_evt = "MIC captured\npress MIC again to redo".to_string();
            }
        }

        draw(target, &hist, &last_evt);
    }
}

pub fn render_list(
    target: &mut FrameBuffer,
    title: &str,
    items: &[String],
    focus: usize,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let width = bb.size.width;
    let height = bb.size.height;
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
    // 视口行数;start 直接由 focus 推出(focus 滚过首页后整页跟随),
    // 不另存窗口变量。上下双向滚动对称。
    let visible = (((height as i32) - start_y) / (item_h as i32)).max(1) as usize;
    let start = focus.saturating_sub(visible.saturating_sub(1));

    for (i, label) in items.iter().enumerate() {
        if i < start {
            continue;
        }
        let row = i - start;
        let rect = Rectangle::new(
            Point::new(0, start_y + (row as i32) * (item_h as i32)),
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

/// session 列表:蓝底 = 选中(焦点);文字色 = working 白 / waiting(非 working)橙。
/// 与通用 render_list 不同 —— 这里底色表示焦点、文字色表示 working 状态,二者正交
/// (故不复用 render_list 的青色焦点底色)。条目用文泉驿字体(支持中文标题)。
pub fn render_session_list(
    target: &mut FrameBuffer,
    title: &str,
    // items: (label, is_working)
    items: &[(String, bool)],
    focus: usize,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let width = bb.size.width;
    let height = bb.size.height;
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
    let visible = (((height as i32) - start_y) / (item_h as i32)).max(1) as usize;
    let start = focus.saturating_sub(visible.saturating_sub(1));

    for (i, (label, is_working)) in items.iter().enumerate() {
        if i < start {
            continue;
        }
        let row = i - start;
        let rect = Rectangle::new(
            Point::new(0, start_y + (row as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        let is_focus = i == focus;
        // 蓝底 = 选中(焦点);文字色 = working 白 / waiting(非 working)橙。二者正交。
        let bg = if is_focus {
            Some(ColorFormat::CSS_DARK_BLUE)
        } else {
            None
        };
        let color = if *is_working {
            ColorFormat::CSS_WHITE
        } else {
            ColorFormat::CSS_DARK_ORANGE
        };
        // 文泉驿字体:标题可能含中文。标签由 mqtt::session_labels 截到 15 字符,单行不溢出。
        draw_text_cjk(target, label, rect, color, bg, HorizontalAlignment::Left)?;
    }
    flush(target)
}

/// Keyboard 模式的会话列表(样式参考 remote 的 render_session_list:文泉驿字体、
/// 整行底色表状态)。无选择焦点 —— 键盘模式只展示,不做切换。底色沿用 remote
/// 视图的配色语言:深绿 = agent 干活中(work/tool/post);深黄 = 等授权(perm,
/// 高亮提醒);深橙 = 空闲(note/done,同 remote 的 stopped);深红 = 失败(err)。
/// 文字统一白色。
/// 排序:非 active(done/note/perm/err,需要用户关注/已结束)排在前面,active
/// 的往后放 —— 屏幕小,列表可能被截断,要关注的会话必须始终可见。
pub fn render_session_view(
    target: &mut FrameBuffer,
    wifi_on: bool,
    ble_on: bool,
    items: &[crate::sessions::SessionEntry],
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let width = bb.size.width;
    let height = bb.size.height;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_status_bar(target, wifi_on, Some(ble_on))?;

    let item_h = LINE_H + 2;
    let start_y: i32 = STATUS_H as i32 + 2;
    let visible = (((height as i32) - start_y) / (item_h as i32)).max(1) as usize;

    if items.is_empty() {
        draw_text(
            target,
            "No sessions",
            Rectangle::new(Point::new(4, start_y), Size::new(width - 4, LINE_H + 2)),
            ColorFormat::CSS_DARK_GRAY,
            None,
            HorizontalAlignment::Left,
        )?;
    }

    // 稳定分区:非 active 优先,各组内保持 upsert 先后顺序。
    let (attention, active): (Vec<&_>, Vec<&_>) = items.iter().partition(|e| !e.st.is_active());

    for (i, e) in attention
        .iter()
        .chain(active.iter())
        .enumerate()
        .take(visible)
    {
        let rect = Rectangle::new(
            Point::new(0, start_y + (i as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        let bg = if e.st.is_perm() {
            ColorFormat::CSS_DARK_GOLDENROD
        } else if e.st.is_active() {
            ColorFormat::CSS_DARK_GREEN
        } else if e.st == crate::sessions::SessionStatus::Err {
            ColorFormat::CSS_DARK_RED
        } else {
            // 空闲/结束(note/done)用橙,与 remote 会话列表的 stopped 同色。
            ColorFormat::CSS_DARK_ORANGE
        };
        // proj 可含中文(路径末段);后缀 sid 短码区分同项目多会话。
        let label = format!("{} ({})", e.proj, e.sid);
        draw_text_cjk(
            target,
            &label,
            rect,
            ColorFormat::CSS_WHITE,
            Some(bg),
            HorizontalAlignment::Left,
        )?;
    }
    flush(target)
}

/// 旋钮会话选择器(键盘模式,按住旋钮临时覆盖屏幕):行样式与 render_session_view
/// 一致(文泉驿字体、整行底色表状态),行序由调用方给出(sessions::SessionTable::ordered,
/// 与常驻会话视图相同)。选中行不加高亮,用 "》 《" 括起标记(wqy12 gb2312 含此标点),
/// 松开旋钮即向主机聚焦该会话。items 非空由调用方保证;focus 超出可视区时滚动窗口跟随。
pub fn render_knob_picker(
    target: &mut FrameBuffer,
    wifi_on: bool,
    ble_on: bool,
    items: &[&crate::sessions::SessionEntry],
    focus: usize,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let width = bb.size.width;
    let height = bb.size.height;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_status_bar(target, wifi_on, Some(ble_on))?;

    let item_h = LINE_H + 2;
    let start_y: i32 = STATUS_H as i32 + 2;
    let visible = (((height as i32) - start_y) / (item_h as i32)).max(1) as usize;
    // 滚动窗口:选中行始终可见(与 render_list 同策略)。
    let start = focus.saturating_sub(visible.saturating_sub(1));

    if items.is_empty() {
        draw_text(
            target,
            "No sessions",
            Rectangle::new(Point::new(4, start_y), Size::new(width - 4, LINE_H + 2)),
            ColorFormat::CSS_DARK_GRAY,
            None,
            HorizontalAlignment::Left,
        )?;
    }

    for (i, e) in items.iter().enumerate().skip(start).take(visible) {
        let rect = Rectangle::new(
            Point::new(0, start_y + (i as i32) * (item_h as i32)),
            Size::new(width, item_h),
        );
        let bg = if e.st.is_perm() {
            ColorFormat::CSS_DARK_GOLDENROD
        } else if e.st.is_active() {
            ColorFormat::CSS_DARK_GREEN
        } else if e.st == crate::sessions::SessionStatus::Err {
            ColorFormat::CSS_DARK_RED
        } else {
            // 空闲/结束(note/done)用橙,与 remote 会话列表的 stopped 同色。
            ColorFormat::CSS_DARK_ORANGE
        };
        // proj 可含中文(路径末段);后缀 sid 短码区分同项目多会话。
        let mut label = format!("{} ({})", e.proj, e.sid);
        if i == focus {
            label = format!("》{label}《");
        }
        draw_text_cjk(
            target,
            &label,
            rect,
            ColorFormat::CSS_WHITE,
            Some(bg),
            HorizontalAlignment::Left,
        )?;
    }
    flush(target)
}

fn render_password(
    target: &mut FrameBuffer,
    header: &str,
    password: &str,
    focus: usize,
) -> anyhow::Result<()> {
    let width = target.bounding_box().size.width;
    let height = target.bounding_box().size.height;
    clear(target, ColorFormat::CSS_BLACK)?;
    draw_text(
        target,
        header,
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
    // 插入点光标:量出密码文本像素宽,在末尾画一个块状光标,随输入/退格左右移动。
    let text_w = Text::new(
        password,
        Point::zero(),
        MonoTextStyle::new(&FONT_7X13_BOLD, ColorFormat::CSS_WHITE),
    )
    .bounding_box()
    .size
    .width;
    fill_rect(
        target,
        Rectangle::new(Point::new(4 + text_w as i32, 18), Size::new(7, 13)),
        ColorFormat::CSS_WHITE,
    )?;
    // 字符轮盘:一排字符,中间高亮(= focus),Next 键/旋钮滑动
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
    // 底部操作提示(贴底,不与字符轮重叠)。
    let hint_y = (height as i32) - LINE_H as i32 - 2;
    draw_text(
        target,
        "BkSp=del ESC=ok",
        Rectangle::new(Point::new(4, hint_y), Size::new(width - 4, LINE_H + 2)),
        ColorFormat::CSS_WHEAT,
        None,
        HorizontalAlignment::Left,
    )?;
    flush(target)
}

// ========== 模式外壳(键盘 / Remote) ==========

const STATUS_H: u32 = 12;

/// 顶部状态栏:蓝牙 / WiFi 连接状态。
pub fn draw_status_bar(
    target: &mut FrameBuffer,
    wifi_on: bool,
    ble_on: Option<bool>,
) -> anyhow::Result<()> {
    let bb = target.bounding_box();
    let bar = Rectangle::new(Point::new(0, 0), Size::new(bb.size.width, STATUS_H));
    fill_rect(target, bar, ColorFormat::CSS_DARK_SLATE_GRAY)?;
    let icon_w: u32 = 12;
    let icon_h: u32 = 8; // 1x open_iconic = 8px;竖直居中放在 STATUS_H 内
    let icon_top: i32 = ((STATUS_H - icon_h) / 2) as i32;
    let gap: u32 = 4;
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
            Point::new(x as i32 + icon_w as i32 / 2, icon_top),
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
        Point::new(x as i32 + icon_w as i32 / 2, icon_top),
        if wifi_on {
            ColorFormat::CSS_WHITE
        } else {
            ColorFormat::CSS_GRAY
        },
    )?;
    Ok(())
}

/// 键盘模式视图:状态栏 + 动画区背景(BLE 连接=绿/未连=红)+ 反馈文字。
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
    // 同 render_keyboard_view:把这帧快照成背景,否则 flush() 之后 buffers 被重置成
    // clear() 的纯黑,后续增量绘制会画到纯黑缓冲上。
    target.fix_background()?;
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
        self.show_with_border(target, text, ColorFormat::CSS_WHITE)
    }

    /// 显示弹窗,指定边框颜色(connecting=黄 / listening=绿 等)。若已打开则只重画(不重复 backup)。
    pub fn show_with_border(
        &mut self,
        target: &mut FrameBuffer,
        text: &str,
        border: ColorFormat,
    ) -> anyhow::Result<()> {
        if self.backup.is_none() {
            self.backup = Some(backup_rect(target, self.rect));
        }
        draw_popup_with_border(target, self.rect, text, border)?;
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

    /// 一次性弹窗:画描边+文字、只推弹窗区域,**不 backup**。
    /// 给 OTA / ClearConfig 这种「弹一下马上重启」的场景用 —— 之后整屏会重新渲染,无需 restore。
    pub fn show_transient(&mut self, target: &mut FrameBuffer, text: &str) -> anyhow::Result<()> {
        draw_popup(target, self.rect, text)?;
        target.flush_rect(self.rect)?;
        Ok(())
    }
}

/// 便捷构造:按给定 bounding_box 居中弹窗(调用方传 `fb.bounding_box()`)。
pub fn popup_centered(bb: Rectangle) -> Popup {
    Popup::new_centered(bb)
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
    draw_popup_with_border(target, rect, text, ColorFormat::CSS_WHITE)
}

fn draw_popup_with_border(
    target: &mut FrameBuffer,
    rect: Rectangle,
    text: &str,
    border: ColorFormat,
) -> anyhow::Result<()> {
    let r = rect.intersection(&target.bounding_box());
    fill_rect(target, r, ColorFormat::CSS_BLACK)?;
    // TUI 风格描边
    r.draw_styled(&PrimitiveStyle::with_stroke(border, 1), target)?;
    // 文字内缩 2px,留出描边
    let inner = Rectangle::new(
        Point::new(r.top_left.x + 2, r.top_left.y + 2),
        Size::new(
            r.size.width.saturating_sub(4),
            r.size.height.saturating_sub(4),
        ),
    );
    draw_text_cjk(
        target,
        text,
        inner,
        ColorFormat::CSS_WHITE,
        Some(ColorFormat::CSS_BLACK),
        HorizontalAlignment::Center,
    )?;
    Ok(())
}

// ========== ASR 文本编辑器 ==========
//
// 远程模式里 ASR 结果不直接发 MQTT,先进这个编辑器:文本带光标(高亮)显示,
// 滚轮左右移光标、退格删字、再按 MIC 在光标处插入新一轮 ASR、Accept 才提交。
// 样式与 ui.rs 弹窗一致(黑底白框),不复用 lcd::UI 那套(带麦克风状态条、风格不同)。
// 光标高亮靠 ansi_plugin:把光标处字符包进 `\x1b[44m…\x1b[49m`,渲染时用 lcd::MyTextStyle
// 这套「U8g2TextStyle + 背景色」桥接(纯渲染原语,不带 lcd::UI 的那一套界面)。

pub struct AsrEditor {
    text: String,
    /// 光标位置,字符索引(不是字节),支持中文等多字节字符。
    cursor: usize,
}

impl AsrEditor {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    /// 在光标处插入一段文本,光标移到插入段之后。
    pub fn insert_str(&mut self, s: &str) {
        let byte_pos = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len());
        self.text.insert_str(byte_pos, s);
        self.cursor += s.chars().count();
    }

    /// 删除光标前一个字符(按字符算,中文删一个字)。
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte_pos = self
            .text
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.remove(byte_pos);
        self.cursor -= 1;
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.text.chars().count() {
            self.cursor += 1;
        }
    }

    /// 取走全部文本并清空(Accept 提交时用)。
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// 全量重绘编辑器:清屏 → 白框 → 标题 → 正文(光标高亮)→ 底部提示。
    pub fn render(&self, target: &mut FrameBuffer) -> anyhow::Result<()> {
        let bb = target.bounding_box();
        let w = bb.size.width as i32;
        let h = bb.size.height as i32;

        clear(target, ColorFormat::CSS_BLACK)?;

        // 外框:内缩 2px,白色 1px 描边(与 draw_popup 同款 TUI 描边)。
        let outer = Rectangle::new(
            Point::new(2, 2),
            Size::new((w - 4).max(0) as u32, (h - 4).max(0) as u32),
        );
        outer.draw_styled(
            &PrimitiveStyle::with_stroke(ColorFormat::CSS_WHITE, 1),
            target,
        )?;

        // 标题
        let title_rect = Rectangle::new(
            Point::new(4, 3),
            Size::new((w - 8).max(0) as u32, LINE_H + 2),
        );
        draw_text(
            target,
            "ASR",
            title_rect,
            ColorFormat::CSS_WHEAT,
            None,
            HorizontalAlignment::Left,
        )?;

        // 正文区:标题下方 ~ 提示上方
        let content_top = 3 + LINE_H as i32 + 2;
        let hint_h = LINE_H as i32 + 4;
        let content_h = (h - 4 - content_top - hint_h).max(LINE_H as i32) as u32;
        let content_rect = Rectangle::new(
            Point::new(4, content_top),
            Size::new((w - 8).max(0) as u32, content_h),
        );

        let display = self.cursor_text();
        let style = TextBoxStyleBuilder::new()
            .alignment(HorizontalAlignment::Left)
            .height_mode(HeightMode::ShrinkToText(VerticalOverdraw::FullRowsOnly))
            .line_height(LineHeight::Pixels(LINE_H))
            .build();
        TextBox::with_textbox_style(
            &display,
            content_rect,
            crate::lcd::MyTextStyle {
                font_style: U8g2TextStyle::new(u8g2_font_wqy12_t_gb2312, ColorFormat::CSS_WHITE),
                vertical_offset: 3,
                bg_color: None,
            },
            style,
        )
        .add_plugin(crate::ansi_plugin::MyAnsiPlugin::new())
        .draw(target)?;

        // 底部提示
        let hint_rect = Rectangle::new(
            Point::new(4, h - 4 - LINE_H as i32 - 1),
            Size::new((w - 8).max(0) as u32, LINE_H + 2),
        );
        draw_text(
            target,
            "Accept=Send  Esc=Cancel  Wheel=Move",
            hint_rect,
            ColorFormat::CSS_WHEAT,
            None,
            HorizontalAlignment::Left,
        )?;

        target.flush()?;
        Ok(())
    }

    /// 把光标处字符包进 ANSI 蓝底转义;光标在末尾时补一个蓝底空格(块状光标)。
    fn cursor_text(&self) -> String {
        let chars: Vec<char> = self.text.chars().collect();
        let mut s = String::with_capacity(self.text.len() + 16);
        for (i, c) in chars.iter().enumerate() {
            if i == self.cursor {
                s.push_str(&format!("\x1b[44m{}\x1b[49m", c));
            } else {
                s.push(*c);
            }
        }
        if self.cursor >= chars.len() {
            s.push_str("\x1b[44m \x1b[49m");
        }
        s
    }
}

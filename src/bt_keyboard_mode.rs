// originally: https://github.com/T-vK/ESP32-BLE-Keyboard
#![allow(dead_code)]

use esp32_nimble::{
    enums::*,
    hid::*,
    utilities::{mutex::Mutex, BleUuid},
    uuid128, BLEAdvertisementData, BLECharacteristic, BLEDevice, BLEHIDDevice, NimbleProperties,
};
use std::sync::Arc;
use zerocopy::IntoBytes;
use zerocopy_derive::{Immutable, IntoBytes};

// const uint8_t KEY_TAB = 0xB3;
// const uint8_t KEY_RETURN = 0xB0;
// const uint8_t KEY_ESC = 0xB1;
pub const KEY_RETURN: u8 = 0xb0;
pub const KEY_ESC: u8 = 0xb1;
pub const KEY_TAB: u8 = 0xb3;

// Function keys (F1-F12)
pub const KEY_F1: u8 = 0x3a;
pub const KEY_F2: u8 = 0x3b;
pub const KEY_F3: u8 = 0x3c;
pub const KEY_F4: u8 = 0x3d;
pub const KEY_F5: u8 = 0x3e;
pub const KEY_F6: u8 = 0x3f;
pub const KEY_F7: u8 = 0x40;
pub const KEY_F8: u8 = 0x41;
pub const KEY_F9: u8 = 0x42;
pub const KEY_F10: u8 = 0x43;
pub const KEY_F11: u8 = 0x44;
pub const KEY_F12: u8 = 0x45;

// Key mapping configuration
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub enum KeyAction {
    #[serde(rename = "combo")]
    Combo {
        raw: String,
        modifiers: Vec<String>,
        key: String,
    },
    #[serde(rename = "text")]
    Text { raw: String, value: String },
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct KeymapConfig {
    #[serde(flatten)]
    pub keys: std::collections::HashMap<String, KeyAction>,
}

impl KeymapConfig {
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn load_from_nvs(nvs: &esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<Self> {
        if !nvs.contains("keymap_config")? {
            log::info!("No keymap config found in NVS, using default");
            return Ok(Self::default());
        }

        let keymap_size = nvs.blob_len("keymap_config")?.unwrap_or_default();
        log::info!("Keymap config size in NVS: {} bytes", keymap_size);

        let mut buf = vec![0; keymap_size];
        nvs.get_blob("keymap_config", &mut buf)?;
        let json = String::from_utf8(buf)?;
        Ok(Self::from_json(&json)?)
    }

    pub fn save_to_nvs(&self, nvs: &mut esp_idf_svc::nvs::EspDefaultNvs) -> anyhow::Result<()> {
        let json = self.to_json()?;
        let bytes = json.as_bytes();
        nvs.set_blob("keymap_config", bytes)?;
        log::info!("Keymap config saved to NVS ({} bytes)", bytes.len());
        Ok(())
    }

    pub fn default() -> Self {
        Self {
            keys: std::collections::HashMap::new(),
        }
    }

    /// Get key name from KeysPin index
    pub fn get_key_name(pin_index: u8) -> &'static str {
        match pin_index {
            KeysPin::MIC => "MIC",
            KeysPin::CUSTOM => "CUSTOM",
            KeysPin::ESC => "ESC",
            KeysPin::NEXT => "NEXT",
            KeysPin::BACKSPACE => "BACKSPACE",
            KeysPin::SWITCH => "SWITCH",
            KeysPin::ACCEPT => "ACCEPT",
            KeysPin::ROTATE_BUTTON => "ROTATE",
            _ => "UNKNOWN",
        }
    }

    /// Merge with another keymap, new values override existing ones
    pub fn merge(&mut self, other: KeymapConfig) {
        for (key, value) in other.keys {
            self.keys.insert(key, value);
        }
    }

    /// Remove a key mapping by name
    pub fn remove(&mut self, key_name: &str) {
        self.keys.remove(key_name);
    }
}

const KEYBOARD_ID: u8 = 0x01;
const MEDIA_KEYS_ID: u8 = 0x02;
const MOUSE_ID: u8 = 0x03;

const HID_KEYBOARD_REPORT_DISCRIPTOR: &[u8] = hid!(
    (USAGE_PAGE, 0x01), // USAGE_PAGE (Generic Desktop Ctrls)
    (USAGE, 0x06),      // USAGE (Keyboard)
    (COLLECTION, 0x01), // COLLECTION (Application)
    // ------------------------------------------------- Keyboard
    (REPORT_ID, KEYBOARD_ID), //   REPORT_ID (1)
    (USAGE_PAGE, 0x07),       //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0xE0),    //   USAGE_MINIMUM (0xE0)
    (USAGE_MAXIMUM, 0xE7),    //   USAGE_MAXIMUM (0xE7)
    (LOGICAL_MINIMUM, 0x00),  //   LOGICAL_MINIMUM (0)
    (LOGICAL_MAXIMUM, 0x01),  //   Logical Maximum (1)
    (REPORT_SIZE, 0x01),      //   REPORT_SIZE (1)
    (REPORT_COUNT, 0x08),     //   REPORT_COUNT (8)
    (HIDINPUT, 0x02), //   INPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x01), //   REPORT_COUNT (1) ; 1 byte (Reserved)
    (REPORT_SIZE, 0x08), //   REPORT_SIZE (8)
    (HIDINPUT, 0x01), //   INPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x05), //   REPORT_COUNT (5) ; 5 bits (Num lock, Caps lock, Scroll lock, Compose, Kana)
    (REPORT_SIZE, 0x01),  //   REPORT_SIZE (1)
    (USAGE_PAGE, 0x08),   //   USAGE_PAGE (LEDs)
    (USAGE_MINIMUM, 0x01), //   USAGE_MINIMUM (0x01) ; Num Lock
    (USAGE_MAXIMUM, 0x05), //   USAGE_MAXIMUM (0x05) ; Kana
    (HIDOUTPUT, 0x02), //   OUTPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x01), //   REPORT_COUNT (1) ; 3 bits (Padding)
    (REPORT_SIZE, 0x03), //   REPORT_SIZE (3)
    (HIDOUTPUT, 0x01), //   OUTPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x06), //   REPORT_COUNT (6) ; 6 bytes (Keys)
    (REPORT_SIZE, 0x08), //   REPORT_SIZE(8)
    (LOGICAL_MINIMUM, 0x00), //   LOGICAL_MINIMUM(0)
    (LOGICAL_MAXIMUM, 0x65), //   LOGICAL_MAXIMUM(0x65) ; 101 keys
    (USAGE_PAGE, 0x07), //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0x00), //   USAGE_MINIMUM (0)
    (USAGE_MAXIMUM, 0x65), //   USAGE_MAXIMUM (0x65)
    (HIDINPUT, 0x00),  //   INPUT (Data,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (END_COLLECTION),  // END_COLLECTION
    (END_COLLECTION)   // END_COLLECTION
);

const HID_REPORT_DISCRIPTOR: &[u8] = hid!(
    (USAGE_PAGE, 0x01), // USAGE_PAGE (Generic Desktop Ctrls)
    (USAGE, 0x06),      // USAGE (Keyboard)
    (COLLECTION, 0x01), // COLLECTION (Application)
    // ------------------------------------------------- Keyboard
    (REPORT_ID, KEYBOARD_ID), //   REPORT_ID (1)
    (USAGE_PAGE, 0x07),       //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0xE0),    //   USAGE_MINIMUM (0xE0)
    (USAGE_MAXIMUM, 0xE7),    //   USAGE_MAXIMUM (0xE7)
    (LOGICAL_MINIMUM, 0x00),  //   LOGICAL_MINIMUM (0)
    (LOGICAL_MAXIMUM, 0x01),  //   Logical Maximum (1)
    (REPORT_SIZE, 0x01),      //   REPORT_SIZE (1)
    (REPORT_COUNT, 0x08),     //   REPORT_COUNT (8)
    (HIDINPUT, 0x02), //   INPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x01), //   REPORT_COUNT (1) ; 1 byte (Reserved)
    (REPORT_SIZE, 0x08), //   REPORT_SIZE (8)
    (HIDINPUT, 0x01), //   INPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x05), //   REPORT_COUNT (5) ; 5 bits (Num lock, Caps lock, Scroll lock, Compose, Kana)
    (REPORT_SIZE, 0x01),  //   REPORT_SIZE (1)
    (USAGE_PAGE, 0x08),   //   USAGE_PAGE (LEDs)
    (USAGE_MINIMUM, 0x01), //   USAGE_MINIMUM (0x01) ; Num Lock
    (USAGE_MAXIMUM, 0x05), //   USAGE_MAXIMUM (0x05) ; Kana
    (HIDOUTPUT, 0x02), //   OUTPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x01), //   REPORT_COUNT (1) ; 3 bits (Padding)
    (REPORT_SIZE, 0x03), //   REPORT_SIZE (3)
    (HIDOUTPUT, 0x01), //   OUTPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x06), //   REPORT_COUNT (6) ; 6 bytes (Keys)
    (REPORT_SIZE, 0x08), //   REPORT_SIZE(8)
    (LOGICAL_MINIMUM, 0x00), //   LOGICAL_MINIMUM(0)
    (LOGICAL_MAXIMUM, 0x65), //   LOGICAL_MAXIMUM(0x65) ; 101 keys
    (USAGE_PAGE, 0x07), //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0x00), //   USAGE_MINIMUM (0)
    (USAGE_MAXIMUM, 0x65), //   USAGE_MAXIMUM (0x65)
    (HIDINPUT, 0x00),  //   INPUT (Data,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (END_COLLECTION),  // END_COLLECTION
    // ------------------------------------------------- Media Keys
    (USAGE_PAGE, 0x0C),         // USAGE_PAGE (Consumer)
    (USAGE, 0x01),              // USAGE (Consumer Control)
    (COLLECTION, 0x01),         // COLLECTION (Application)
    (REPORT_ID, MEDIA_KEYS_ID), //   REPORT_ID (3)
    (USAGE_PAGE, 0x0C),         //   USAGE_PAGE (Consumer)
    (LOGICAL_MINIMUM, 0x00),    //   LOGICAL_MINIMUM (0)
    (LOGICAL_MAXIMUM, 0x01),    //   LOGICAL_MAXIMUM (1)
    (REPORT_SIZE, 0x01),        //   REPORT_SIZE (1)
    (REPORT_COUNT, 0x10),       //   REPORT_COUNT (16)
    (USAGE, 0xB5),              //   USAGE (Scan Next Track)     ; bit 0: 1
    (USAGE, 0xB6),              //   USAGE (Scan Previous Track) ; bit 1: 2
    (USAGE, 0xB7),              //   USAGE (Stop)                ; bit 2: 4
    (USAGE, 0xCD),              //   USAGE (Play/Pause)          ; bit 3: 8
    (USAGE, 0xE2),              //   USAGE (Mute)                ; bit 4: 16
    (USAGE, 0xE9),              //   USAGE (Volume Increment)    ; bit 5: 32
    (USAGE, 0xEA),              //   USAGE (Volume Decrement)    ; bit 6: 64
    (USAGE, 0x23, 0x02),        //   Usage (WWW Home)            ; bit 7: 128
    (USAGE, 0x94, 0x01),        //   Usage (My Computer) ; bit 0: 1
    (USAGE, 0x92, 0x01),        //   Usage (Calculator)  ; bit 1: 2
    (USAGE, 0x2A, 0x02),        //   Usage (WWW fav)     ; bit 2: 4
    (USAGE, 0x21, 0x02),        //   Usage (WWW search)  ; bit 3: 8
    (USAGE, 0x26, 0x02),        //   Usage (WWW stop)    ; bit 4: 16
    (USAGE, 0x24, 0x02),        //   Usage (WWW back)    ; bit 5: 32
    (USAGE, 0x83, 0x01),        //   Usage (Media sel)   ; bit 6: 64
    (USAGE, 0x8A, 0x01),        //   Usage (Mail)        ; bit 7: 128
    (HIDINPUT, 0x02), // INPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (END_COLLECTION), // END_COLLECTION
    // ------------------------------------------------- Mouse
    (USAGE_PAGE, 0x01),    //     USAGE_PAGE (Generic Desktop)
    (USAGE, 0x02),         //     USAGE (Mouse)
    (COLLECTION, 0x01),    //     COLLECTION (Application)
    (USAGE, 0x01),         //       USAGE (Pointer)
    (COLLECTION, 0x00),    //       COLLECTION (Physical)
    (REPORT_ID, MOUSE_ID), //       REPORT_ID (2)
    // ------------------------------------------------- Buttons (Left, Right, Middle, Back, Forward)
    (USAGE_PAGE, 0x09),      //     USAGE_PAGE (Button)
    (USAGE_MINIMUM, 0x01),   //     USAGE_MINIMUM (Button 1)
    (USAGE_MAXIMUM, 0x05),   //     USAGE_MAXIMUM (Button 5)
    (LOGICAL_MINIMUM, 0x00), //     LOGICAL_MINIMUM (0)
    (LOGICAL_MAXIMUM, 0x01), //     LOGICAL_MAXIMUM (1)
    (REPORT_SIZE, 0x01),     //     REPORT_SIZE (1)
    (REPORT_COUNT, 0x05),    //     REPORT_COUNT (5)
    (HIDINPUT, 0x02),        //     INPUT (Data, Variable, Absolute) ;5 button bits
    // ------------------------------------------------- Padding
    (REPORT_SIZE, 0x03),  //     REPORT_SIZE (3)
    (REPORT_COUNT, 0x01), //     REPORT_COUNT (1)
    (HIDINPUT, 0x03),     //     INPUT (Constant, Variable, Absolute) ;3 bit padding
    // ------------------------------------------------- X/Y position, Wheel
    (USAGE_PAGE, 0x01),      //     USAGE_PAGE (Generic Desktop)
    (USAGE, 0x30),           //     USAGE (X)
    (USAGE, 0x31),           //     USAGE (Y)
    (USAGE, 0x38),           //     USAGE (Wheel)
    (LOGICAL_MINIMUM, 0x81), //     LOGICAL_MINIMUM (-127)
    (LOGICAL_MAXIMUM, 0x7f), //     LOGICAL_MAXIMUM (127)
    (REPORT_SIZE, 0x08),     //     REPORT_SIZE (8)
    (REPORT_COUNT, 0x03),    //     REPORT_COUNT (3)
    (HIDINPUT, 0x06),        //     INPUT (Data, Variable, Relative) ;3 bytes (X,Y,Wheel)
    // ------------------------------------------------- Horizontal wheel
    (USAGE_PAGE, 0x0c),      // USAGE PAGE (Consumer Devices)
    (USAGE, 0x38, 0x02),     // USAGE (AC Pan)
    (LOGICAL_MINIMUM, 0x81), // LOGICAL_MINIMUM (-127)
    (LOGICAL_MAXIMUM, 0x7f), // LOGICAL_MAXIMUM (127)
    (REPORT_SIZE, 0x08),     // REPORT_SIZE (8)
    (REPORT_COUNT, 0x01),    // REPORT_COUNT (1)
    (HIDINPUT, 0x06),        // INPUT (Data, Var, Rel)
    (END_COLLECTION),        // END_COLLECTION
    (END_COLLECTION)         // END_COLLECTION
);

const SHIFT: u8 = 0x80;
const ASCII_MAP: &[u8] = &[
    0x00,         // NUL
    0x00,         // SOH
    0x00,         // STX
    0x00,         // ETX
    0x00,         // EOT
    0x00,         // ENQ
    0x00,         // ACK
    0x00,         // BEL
    0x2a,         // BS	Backspace
    0x2b,         // TAB	Tab
    0x28,         // LF	Enter
    0x00,         // VT
    0x00,         // FF
    0x00,         // CR
    0x00,         // SO
    0x00,         // SI
    0x00,         // DEL
    0x00,         // DC1
    0x00,         // DC2
    0x00,         // DC3
    0x00,         // DC4
    0x00,         // NAK
    0x00,         // SYN
    0x00,         // ETB
    0x00,         // CAN
    0x00,         // EM
    0x00,         // SUB
    0x29,         // ESC
    0x00,         // FS
    0x00,         // GS
    0x00,         // RS
    0x00,         // US
    0x2c,         //  ' '
    0x1e | SHIFT, // !
    0x34 | SHIFT, // "
    0x20 | SHIFT, // #
    0x21 | SHIFT, // $
    0x22 | SHIFT, // %
    0x24 | SHIFT, // &
    0x34,         // '
    0x26 | SHIFT, // (
    0x27 | SHIFT, // )
    0x25 | SHIFT, // *
    0x2e | SHIFT, // +
    0x36,         // ,
    0x2d,         // -
    0x37,         // .
    0x38,         // /
    0x27,         // 0
    0x1e,         // 1
    0x1f,         // 2
    0x20,         // 3
    0x21,         // 4
    0x22,         // 5
    0x23,         // 6
    0x24,         // 7
    0x25,         // 8
    0x26,         // 9
    0x33 | SHIFT, // :
    0x33,         // ;
    0x36 | SHIFT, // <
    0x2e,         // =
    0x37 | SHIFT, // >
    0x38 | SHIFT, // ?
    0x1f | SHIFT, // @
    0x04 | SHIFT, // A
    0x05 | SHIFT, // B
    0x06 | SHIFT, // C
    0x07 | SHIFT, // D
    0x08 | SHIFT, // E
    0x09 | SHIFT, // F
    0x0a | SHIFT, // G
    0x0b | SHIFT, // H
    0x0c | SHIFT, // I
    0x0d | SHIFT, // J
    0x0e | SHIFT, // K
    0x0f | SHIFT, // L
    0x10 | SHIFT, // M
    0x11 | SHIFT, // N
    0x12 | SHIFT, // O
    0x13 | SHIFT, // P
    0x14 | SHIFT, // Q
    0x15 | SHIFT, // R
    0x16 | SHIFT, // S
    0x17 | SHIFT, // T
    0x18 | SHIFT, // U
    0x19 | SHIFT, // V
    0x1a | SHIFT, // W
    0x1b | SHIFT, // X
    0x1c | SHIFT, // Y
    0x1d | SHIFT, // Z
    0x2f,         // [
    0x31,         // bslash
    0x30,         // ]
    0x23 | SHIFT, // ^
    0x2d | SHIFT, // _
    0x35,         // `
    0x04,         // a
    0x05,         // b
    0x06,         // c
    0x07,         // d
    0x08,         // e
    0x09,         // f
    0x0a,         // g
    0x0b,         // h
    0x0c,         // i
    0x0d,         // j
    0x0e,         // k
    0x0f,         // l
    0x10,         // m
    0x11,         // n
    0x12,         // o
    0x13,         // p
    0x14,         // q
    0x15,         // r
    0x16,         // s
    0x17,         // t
    0x18,         // u
    0x19,         // v
    0x1a,         // w
    0x1b,         // x
    0x1c,         // y
    0x1d,         // z
    0x2f | SHIFT, // {
    0x31 | SHIFT, // |
    0x30 | SHIFT, // }
    0x35 | SHIFT, // ~
    0,            // DEL
];

const KEY_MEDIA_NEXT_TRACK: [u8; 2] = [1, 0];
const KEY_MEDIA_PREVIOUS_TRACK: [u8; 2] = [2, 0];
const KEY_MEDIA_STOP: [u8; 2] = [4, 0];
const KEY_MEDIA_PLAY_PAUSE: [u8; 2] = [8, 0];
const KEY_MEDIA_MUTE: [u8; 2] = [16, 0];
const KEY_MEDIA_VOLUME_UP: [u8; 2] = [32, 0];
const KEY_MEDIA_VOLUME_DOWN: [u8; 2] = [64, 0];
const KEY_MEDIA_WWW_HOME: [u8; 2] = [128, 0];
const KEY_MEDIA_LOCAL_MACHINE_BROWSER: [u8; 2] = [0, 1]; // Opens "My Computer" on Windows
const KEY_MEDIA_CALCULATOR: [u8; 2] = [0, 2];
const KEY_MEDIA_WWW_BOOKMARKS: [u8; 2] = [0, 4];
const KEY_MEDIA_WWW_SEARCH: [u8; 2] = [0, 8];

const MOUSE_LEFT: u8 = 1;
const MOUSE_RIGHT: u8 = 2;
const MOUSE_MIDDLE: u8 = 4;
const MOUSE_BACK: u8 = 8;
const MOUSE_FORWARD: u8 = 16;
const MOUSE_ALL: u8 = MOUSE_LEFT | MOUSE_RIGHT | MOUSE_MIDDLE | MOUSE_BACK | MOUSE_FORWARD;

#[derive(IntoBytes, Immutable)]
#[repr(packed)]
struct KeyReport {
    modifiers: u8,
    reserved: u8,
    keys: [u8; 6],
}

#[derive(IntoBytes, Immutable)]
#[repr(packed)]
struct MediaKeyReport {
    keys: [u8; 2],
}

pub struct KeysPin {
    pub mic: crate::AnyBtn,
    pub custom: crate::AnyBtn,
    pub esc: crate::AnyBtn,
    pub next: crate::AnyBtn,
    pub backspace: crate::AnyBtn,
    pub switch: crate::AnyBtn,
    pub accept: crate::AnyBtn,
    pub rotate_a: crate::AnyBtn,
    pub rotate_b: crate::AnyBtn,
    pub rotate_button: crate::AnyBtn,
}

impl KeysPin {
    pub const MIC: u8 = 0;
    pub const CUSTOM: u8 = 1;
    pub const ESC: u8 = 2;
    pub const NEXT: u8 = 3;
    pub const BACKSPACE: u8 = 4;
    pub const SWITCH: u8 = 5;
    pub const ACCEPT: u8 = 6;
    pub const ROTATE_BUTTON: u8 = 7;
}

pub async fn key_event(
    key_pins: &mut KeysPin,
    rx: &mut tokio::sync::mpsc::Receiver<ControllerCommand>,
) -> ControllerCommand {
    tokio::select! {
        _ = key_pins.mic.wait_for_any_edge() => {
            if key_pins.mic.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::MIC)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::MIC)
            }
        },
        _ = key_pins.custom.wait_for_any_edge() => {
            if key_pins.custom.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::CUSTOM)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::CUSTOM)
            }
        }
        _ = key_pins.esc.wait_for_any_edge() => {
            if key_pins.esc.is_low() {
                log::info!("ESC key pressed");
                ControllerCommand::KeyboardPress(KeysPin::ESC)
            } else {
                log::info!("ESC key released");
                ControllerCommand::KeyboardRelease(KeysPin::ESC)
            }
        },
        _ = key_pins.next.wait_for_any_edge() => {
            if key_pins.next.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::NEXT)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::NEXT)
            }
        },
        _ = key_pins.switch.wait_for_any_edge() => {
            if key_pins.switch.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::SWITCH)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::SWITCH)
            }
        },
        _ = key_pins.backspace.wait_for_any_edge() => {
            if key_pins.backspace.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::BACKSPACE)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::BACKSPACE)
            }
        },
        _ = key_pins.accept.wait_for_any_edge() => {
            if key_pins.accept.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::ACCEPT)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::ACCEPT)
            }
        },
        _ = key_pins.rotate_a.wait_for_any_edge() => {
            if key_pins.rotate_a.is_high() {
                if key_pins.rotate_b.is_low() {
                    ControllerCommand::RotateDown
                } else {
                    ControllerCommand::RotateUp
                }
            } else {
                if key_pins.rotate_b.is_low() {
                    ControllerCommand::RotateUp
                } else {
                    ControllerCommand::RotateDown
                }
            }
        },
        _ = key_pins.rotate_button.wait_for_any_edge() => {
            if key_pins.rotate_button.is_low() {
                ControllerCommand::KeyboardPress(KeysPin::ROTATE_BUTTON)
            } else {
                ControllerCommand::KeyboardRelease(KeysPin::ROTATE_BUTTON)
            }
        },
        Some(event) = rx.recv() => {
            event
        }
    }
}

pub struct Keyboard {
    hid_service_id: BleUuid,
    input_keyboard: Arc<Mutex<BLECharacteristic>>,
    output_keyboard: Arc<Mutex<BLECharacteristic>>,
    key_report: KeyReport,
}

impl Keyboard {
    pub fn new(device: &mut BLEDevice, battery_level: u8) -> anyhow::Result<Self> {
        device
            .security()
            .set_auth(AuthReq::all())
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let server = device.get_server();
        server.on_connect(|_server, _client| {});
        let mut hid = BLEHIDDevice::new(server);

        let input_keyboard = hid.input_report(KEYBOARD_ID);
        let output_keyboard = hid.output_report(KEYBOARD_ID);

        hid.manufacturer("VibeKeys-MAX");
        hid.pnp(0x02, 0x2E8A, 0x820a, 0x0210);
        hid.hid_info(0x00, 0x01);

        hid.report_map(HID_KEYBOARD_REPORT_DISCRIPTOR);

        hid.set_battery_level(battery_level);

        let hid_service_id = hid.hid_service().lock().uuid();

        Ok(Self {
            hid_service_id,
            input_keyboard,
            output_keyboard,
            key_report: KeyReport {
                modifiers: 0,
                reserved: 0,
                keys: [0; 6],
            },
        })
    }

    pub fn write(&mut self, str: &str) {
        for char in str.as_bytes() {
            self.press(*char);
            self.release();
        }
    }

    pub fn ctrl_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x01;
        self.press(char);
    }

    pub fn r_ctrl_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x10;
        self.press(char);
    }

    pub fn shift_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x02;
        self.press(char);
    }

    pub fn r_shift_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x20;
        self.press(char);
    }

    pub fn alt_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x04;
        self.press(char);
    }

    pub fn gui_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x08;
        self.press(char);
    }

    pub fn press(&mut self, char: u8) {
        if char > ASCII_MAP.len() as u8 {
            self.key_report.keys[0] = char;
            self.send_report(&self.key_report);
            return;
        }

        let mut key = ASCII_MAP[char as usize];
        if (key & SHIFT) > 0 {
            self.key_report.modifiers |= 0x02;
            key &= !SHIFT;
        }
        self.key_report.keys[0] = key;
        self.send_report(&self.key_report);
    }

    pub fn press_raw(&mut self, key: u8, modifiers: u8) {
        self.key_report.modifiers = modifiers;
        self.key_report.keys[0] = key;
        self.send_report(&self.key_report);
    }

    pub fn release(&mut self) {
        self.key_report.modifiers = 0;
        self.key_report.keys.fill(0);
        self.send_report(&self.key_report);
    }

    fn send_report(&self, keys: &KeyReport) {
        self.input_keyboard
            .lock()
            .set_value(keys.as_bytes())
            .notify();
        esp_idf_svc::hal::delay::Ets::delay_ms(7);
    }

    pub fn hid_service_id(&self) -> BleUuid {
        self.hid_service_id
    }
}

pub struct KeyboardAndMouse {
    hid_service_id: BleUuid,
    input_keyboard: Arc<Mutex<BLECharacteristic>>,
    output_keyboard: Arc<Mutex<BLECharacteristic>>,
    input_media_keys: Arc<Mutex<BLECharacteristic>>,
    input_mouse: Arc<Mutex<BLECharacteristic>>,
    key_report: KeyReport,
    media_key_report: MediaKeyReport,
}

impl KeyboardAndMouse {
    pub fn new(device: &mut BLEDevice, battery_level: u8) -> anyhow::Result<Self> {
        device
            .security()
            .set_auth(AuthReq::all())
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let server = device.get_server();
        server.on_connect(|_server, _client| {});
        let mut hid = BLEHIDDevice::new(server);

        let input_keyboard = hid.input_report(KEYBOARD_ID);
        let output_keyboard = hid.output_report(KEYBOARD_ID);
        let input_media_keys = hid.input_report(MEDIA_KEYS_ID);
        let input_mouse = hid.input_report(MOUSE_ID);

        hid.manufacturer("VibeKeys-MAX");
        hid.pnp(0x02, 0x2E8A, 0x820a, 0x0210);
        hid.hid_info(0x00, 0x01);

        hid.report_map(HID_REPORT_DISCRIPTOR);

        hid.set_battery_level(battery_level);

        let hid_service_id = hid.hid_service().lock().uuid();

        Ok(Self {
            hid_service_id,
            input_keyboard,
            output_keyboard,
            input_media_keys,
            input_mouse,
            key_report: KeyReport {
                modifiers: 0,
                reserved: 0,
                keys: [0; 6],
            },
            media_key_report: MediaKeyReport { keys: [0; 2] },
        })
    }

    pub fn write(&mut self, str: &str) {
        for char in str.as_bytes() {
            self.press(*char);
            self.release();
        }
    }

    pub fn ctrl_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x01;
        self.press(char);
    }

    pub fn r_ctrl_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x10;
        self.press(char);
    }

    pub fn shift_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x02;
        self.press(char);
    }

    pub fn r_shift_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x20;
        self.press(char);
    }

    pub fn alt_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x04;
        self.press(char);
    }

    pub fn gui_press(&mut self, char: u8) {
        self.key_report.modifiers |= 0x08;
        self.press(char);
    }

    pub fn press(&mut self, char: u8) {
        if char > ASCII_MAP.len() as u8 {
            self.key_report.keys[0] = char;
            self.send_report(&self.key_report);
            return;
        }

        let mut key = ASCII_MAP[char as usize];
        if (key & SHIFT) > 0 {
            self.key_report.modifiers |= 0x02;
            key &= !SHIFT;
        }
        self.key_report.keys[0] = key;
        self.send_report(&self.key_report);
    }

    pub fn press_raw(&mut self, key: u8, modifiers: u8) {
        self.key_report.modifiers = modifiers;
        self.key_report.keys[0] = key;
        self.send_report(&self.key_report);
    }

    pub fn release(&mut self) {
        self.key_report.modifiers = 0;
        self.key_report.keys.fill(0);
        self.send_report(&self.key_report);
    }

    fn send_report(&self, keys: &KeyReport) {
        self.input_keyboard
            .lock()
            .set_value(keys.as_bytes())
            .notify();
        esp_idf_svc::hal::delay::Ets::delay_ms(7);
    }

    pub fn send_media_report(&mut self, keys: [u8; 2]) {
        let k_16 = (keys[1] as u16) | ((keys[0] as u16) << 8);
        let mut media_key_report_16 =
            (self.media_key_report.keys[1] as u16) | ((self.media_key_report.keys[0] as u16) << 8);
        media_key_report_16 |= k_16;
        self.media_key_report.keys[0] = ((media_key_report_16 & 0xFF00) >> 8) as u8;
        self.media_key_report.keys[1] = (media_key_report_16 & 0x00FF) as u8;
        self.input_media_keys
            .lock()
            .set_value(self.media_key_report.as_bytes())
            .notify();
        esp_idf_svc::hal::delay::Ets::delay_ms(7);
    }

    pub fn release_media(&mut self, keys: [u8; 2]) {
        let k_16 = (keys[1] as u16) | ((keys[0] as u16) << 8);
        let mut media_key_report_16 =
            (self.media_key_report.keys[1] as u16) | ((self.media_key_report.keys[0] as u16) << 8);
        media_key_report_16 &= !k_16;
        self.media_key_report.keys[0] = ((media_key_report_16 & 0xFF00) >> 8) as u8;
        self.media_key_report.keys[1] = (media_key_report_16 & 0x00FF) as u8;
        self.input_media_keys
            .lock()
            .set_value(self.media_key_report.as_bytes())
            .notify();
        esp_idf_svc::hal::delay::Ets::delay_ms(7);
    }

    pub fn mouse_execute(&mut self, buttons: u8, x: i8, y: i8, wheel: i8, h_wheel: i8) {
        let mouse_report: [u8; 5] = [buttons, x as u8, y as u8, wheel as u8, h_wheel as u8];
        self.input_mouse.lock().set_value(&mouse_report).notify();
        esp_idf_svc::hal::delay::Ets::delay_ms(7);
    }

    pub fn mouse_move(&mut self, x: i8, y: i8, wheel: i8, h_wheel: i8) {
        self.mouse_execute(0, x, y, wheel, h_wheel);
    }

    pub fn mouse_click(&mut self, button: u8) {
        self.mouse_execute(button, 0, 0, 0, 0);
        self.mouse_execute(0, 0, 0, 0, 0);
    }

    pub fn hid_service_id(&self) -> BleUuid {
        self.hid_service_id
    }
}

// controller service and characteristic UUIDs

const CONTROLLER_SERVICE_ID: BleUuid = uuid128!("9c80ffb6-affa-4083-944a-91e34c88bd76");
const KEYBOARD_DISPLAY_ID: BleUuid = uuid128!("cdaa6472-67a8-4241-93cf-145051608573");
const KEYBOARD_NOTIFY_ID: BleUuid = uuid128!("d4f7e1b3-3c4d-4f4e-8e2a-8f4e5c6d7e8f");
const KEYMAP_CONFIG_ID: BleUuid = uuid128!("6f2a291c-0e4d-4f0f-9446-50bcd0b73bb0");

pub struct ControllerService {
    pub notify_characteristic: Arc<Mutex<BLECharacteristic>>,
}

impl ControllerService {
    pub fn notify(&self, message: &str) {
        self.notify_characteristic
            .lock()
            .set_value(message.as_bytes())
            .notify();
    }
}

#[derive(Debug)]
pub enum ControllerCommand {
    DisplayKeyboard(String),
    KeyboardPress(u8),
    KeyboardRelease(u8),
    RotateDown,
    RotateUp,
    KeymapConfig(String),
}

pub fn new_controller_service(
    device: &mut BLEDevice,
    tx: tokio::sync::mpsc::Sender<ControllerCommand>,
) -> anyhow::Result<ControllerService> {
    let server = device.get_server();
    let service = server.create_service(CONTROLLER_SERVICE_ID);

    let tx_ = tx.clone();

    let display_characteristic = service
        .lock()
        .create_characteristic(KEYBOARD_DISPLAY_ID, NimbleProperties::WRITE);

    display_characteristic.lock().on_write(move |args| {
        log::info!("Wrote to controller display characteristic");
        let data = args.recv_data();
        log::info!("Received data: {:?}", data);
        let s = String::from_utf8_lossy(&data).to_string();

        let _ = tx_.blocking_send(ControllerCommand::DisplayKeyboard(s));
    });

    let notify_characteristic = service
        .lock()
        .create_characteristic(KEYBOARD_NOTIFY_ID, NimbleProperties::NOTIFY);

    let keymap_config_characteristic = service
        .lock()
        .create_characteristic(KEYMAP_CONFIG_ID, NimbleProperties::WRITE);

    keymap_config_characteristic.lock().on_write(move |args| {
        log::info!("Wrote to keymap config characteristic");
        let data = args.recv_data();
        log::info!("Received keymap data: {:?}", data);
        let s = String::from_utf8_lossy(&data).to_string();

        let _ = tx.blocking_send(ControllerCommand::KeymapConfig(s));
    });

    Ok(ControllerService {
        notify_characteristic,
    })
}

pub fn start_ble_advertising(
    device: &mut BLEDevice,
    hid_service_id: BleUuid,
) -> anyhow::Result<()> {
    let ble_advertising = device.get_advertising();
    ble_advertising.lock().scan_response(true).set_data(
        BLEAdvertisementData::new()
            .name("VibeKeys-MAX")
            .appearance(0x03C1)
            .add_service_uuid(hid_service_id)
            .add_service_uuid(CONTROLLER_SERVICE_ID),
    )?;
    ble_advertising.lock().start()?;

    Ok(())
}

/// Convert key name string to HID keycode with optional modifier bit
/// Returns (keycode, modifier_bit)
/// For modifier keys (Ctrl, Shift, Alt, GUI), modifier_bit is non-zero
pub fn key_name_to_hid_code(key: &str) -> anyhow::Result<(u8, u8)> {
    let key_upper = key.to_uppercase();
    let (code, modifier) = match key_upper.as_str() {
        // Letters A-Z
        "A" => (0x04, 0),
        "B" => (0x05, 0),
        "C" => (0x06, 0),
        "D" => (0x07, 0),
        "E" => (0x08, 0),
        "F" => (0x09, 0),
        "G" => (0x0A, 0),
        "H" => (0x0B, 0),
        "I" => (0x0C, 0),
        "J" => (0x0D, 0),
        "K" => (0x0E, 0),
        "L" => (0x0F, 0),
        "M" => (0x10, 0),
        "N" => (0x11, 0),
        "O" => (0x12, 0),
        "P" => (0x13, 0),
        "Q" => (0x14, 0),
        "R" => (0x15, 0),
        "S" => (0x16, 0),
        "T" => (0x17, 0),
        "U" => (0x18, 0),
        "V" => (0x19, 0),
        "W" => (0x1A, 0),
        "X" => (0x1B, 0),
        "Y" => (0x1C, 0),
        "Z" => (0x1D, 0),
        // Numbers
        "1" => (0x1E, 0),
        "2" => (0x1F, 0),
        "3" => (0x20, 0),
        "4" => (0x21, 0),
        "5" => (0x22, 0),
        "6" => (0x23, 0),
        "7" => (0x24, 0),
        "8" => (0x25, 0),
        "9" => (0x26, 0),
        "0" => (0x27, 0),
        // Special keys
        "ENTER" | "RETURN" => (0x28, 0),
        "ESCAPE" | "ESC" => (0x29, 0),
        "BACKSPACE" => (0x2A, 0),
        "TAB" => (0x2B, 0),
        "SPACE" => (0x2C, 0),
        // F keys
        "F1" => (KEY_F1, 0),
        "F2" => (KEY_F2, 0),
        "F3" => (KEY_F3, 0),
        "F4" => (KEY_F4, 0),
        "F5" => (KEY_F5, 0),
        "F6" => (KEY_F6, 0),
        "F7" => (KEY_F7, 0),
        "F8" => (KEY_F8, 0),
        "F9" => (KEY_F9, 0),
        "F10" => (KEY_F10, 0),
        "F11" => (KEY_F11, 0),
        "F12" => (KEY_F12, 0),
        // Arrow keys
        "RIGHT" => (0x4F, 0),
        "LEFT" => (0x50, 0),
        "DOWN" => (0x51, 0),
        "UP" => (0x52, 0),
        // Symbols
        "MINUS" | "PLUS" => (0x2D, 0),
        "EQUAL" => (0x2E, 0),
        "SEMICOLON" => (0x33, 0),
        "QUOTE" => (0x34, 0),
        "BACKQUOTE" => (0x35, 0),
        "BACKSLASH" => (0x31, 0),
        "COMMA" => (0x36, 0),
        "PERIOD" => (0x37, 0),
        "SLASH" => (0x38, 0),
        "BRACKETLEFT" => (0x2F, 0),
        "BRACKETRIGHT" => (0x30, 0),
        // Modifier keys - keycode + modifier bit
        "CTRL" | "CONTROL" => (0xE0, 0x01),
        "SHIFT" => (0xE1, 0x02),
        "ALT" | "OPTION" => (0xE2, 0x04),
        "GUI" | "WIN" | "WINDOWS" | "META" | "SUPER" | "COMMAND" => (0xE3, 0x08),
        "RCTRL" | "RCONTROL" => (0xE4, 0x10),
        "RSHIFT" => (0xE5, 0x20),
        "RALT" | "ROPTION" => (0xE6, 0x40),
        "RGUI" | "RWIN" | "RWINDOWS" | "RMETA" | "RSUPER" | "RCOMMAND" => (0xE7, 0x80),
        _ => {
            return Err(anyhow::anyhow!("Unknown key: {}", key));
        }
    };
    Ok((code, modifier))
}

// originally: https://github.com/T-vK/ESP32-BLE-Keyboard
#![allow(dead_code)]

use esp32_nimble::{
    enums::*,
    hid::*,
    utilities::{mutex::Mutex, BleUuid},
    uuid128, BLEAdvertisementData, BLECharacteristic, BLEDevice, BLEHIDDevice, BLEServer,
    NimbleProperties,
};
use esp_idf_svc::hal::gpio::*;
use futures_util::FutureExt;
use std::sync::{mpsc::Sender, Arc};
use zerocopy::IntoBytes;
use zerocopy_derive::{Immutable, IntoBytes};

// const uint8_t KEY_TAB = 0xB3;
// const uint8_t KEY_RETURN = 0xB0;
// const uint8_t KEY_ESC = 0xB1;
pub const KEY_RETURN: u8 = 0xb0;
pub const KEY_ESC: u8 = 0xb1;
pub const KEY_TAB: u8 = 0xb3;

const KEYBOARD_ID: u8 = 0x01;
const MEDIA_KEYS_ID: u8 = 0x02;
const MOUSE_ID: u8 = 0x03;

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

    pub fn release(&mut self) {
        self.key_report.modifiers = 0;
        self.key_report.keys.fill(0);
        self.send_report(&self.key_report);
    }

    pub fn send_report(&self, keys: &KeyReport) {
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
const GOTO_OTA_ID: BleUuid = uuid128!("703df7cd-1fec-4126-b3df-a6a8858c1e5e");
const KEYBOARD_DISPLAY_ID: BleUuid = uuid128!("cdaa6472-67a8-4241-93cf-145051608573");
const KEYBOARD_NOTIFY_ID: BleUuid = uuid128!("d4f7e1b3-3c4d-4f4e-8e2a-8f4e5c6d7e8f");

pub struct ControllerService {
    pub notify_characteristic: Arc<Mutex<BLECharacteristic>>,
    pub rx: std::sync::mpsc::Receiver<ControllerCommand>,
}

impl ControllerService {
    pub fn notify(&self, message: &str) {
        self.notify_characteristic
            .lock()
            .set_value(message.as_bytes())
            .notify();
    }
}

pub enum ControllerCommand {
    GoToOta,
    DisplayKeyboard(String),
    KeyboardPress(u8),
    KeyboardRelease(u8),
}

pub fn new_controller_service(
    device: &mut BLEDevice,
) -> anyhow::Result<(ControllerService, Sender<ControllerCommand>)> {
    let (tx, rx) = std::sync::mpsc::channel::<ControllerCommand>();
    let server = device.get_server();
    let service = server.create_service(CONTROLLER_SERVICE_ID);

    let tx_ = tx.clone();
    let ota_characteristic = service
        .lock()
        .create_characteristic(GOTO_OTA_ID, NimbleProperties::WRITE);

    ota_characteristic.lock().on_write(move |args| {
        log::info!("Wrote to controller OTA characteristic");
        let data = args.recv_data();
        log::info!("Received data: {:?}", data);

        let _ = tx_.send(ControllerCommand::GoToOta);
    });

    let display_characteristic = service
        .lock()
        .create_characteristic(KEYBOARD_DISPLAY_ID, NimbleProperties::WRITE);

    let tx_ = tx.clone();
    display_characteristic.lock().on_write(move |args| {
        log::info!("Wrote to controller display characteristic");
        let data = args.recv_data();
        log::info!("Received data: {:?}", data);
        let s = String::from_utf8_lossy(&data).to_string();

        let _ = tx_.send(ControllerCommand::DisplayKeyboard(s));
    });

    let notify_characteristic = service
        .lock()
        .create_characteristic(KEYBOARD_NOTIFY_ID, NimbleProperties::NOTIFY);

    Ok((
        ControllerService {
            notify_characteristic,
            rx,
        },
        tx,
    ))
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

pub struct KeysPin(
    pub Gpio0,
    pub Gpio1,
    pub Gpio2,
    pub Gpio3,
    pub Gpio4,
    pub Gpio5,
    pub Gpio6,
    pub Gpio7,
    pub Gpio18,
);

pub fn start_key_listen(tx: Sender<ControllerCommand>, keys: KeysPin) -> anyhow::Result<()> {
    let mut button_k0 = esp_idf_svc::hal::gpio::PinDriver::input(keys.0)?;
    button_k0.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k0.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k1 = esp_idf_svc::hal::gpio::PinDriver::input(keys.1)?;
    button_k1.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k1.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k2 = esp_idf_svc::hal::gpio::PinDriver::input(keys.2)?;
    button_k2.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k2.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k3 = esp_idf_svc::hal::gpio::PinDriver::input(keys.3)?;
    button_k3.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k3.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k4 = esp_idf_svc::hal::gpio::PinDriver::input(keys.4)?;
    button_k4.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k4.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k5 = esp_idf_svc::hal::gpio::PinDriver::input(keys.5)?;
    button_k5.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k5.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k6 = esp_idf_svc::hal::gpio::PinDriver::input(keys.6)?;
    button_k6.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k6.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k7 = esp_idf_svc::hal::gpio::PinDriver::input(keys.7)?;
    button_k7.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k7.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    let mut button_k18 = esp_idf_svc::hal::gpio::PinDriver::input(keys.8)?;
    button_k18.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button_k18.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::AnyEdge)?;

    loop {
        let (key, is_low) = esp_idf_svc::hal::task::block_on(async {
            futures_util::select! {
                _ = button_k0.wait_for_any_edge().fuse() => {
                    (0,button_k0.is_low())
                }
                _ = button_k1.wait_for_any_edge().fuse() => {
                    (1,button_k1.is_low())
                },
                _ = button_k2.wait_for_any_edge().fuse() => {
                    (2,button_k2.is_low())
                },
                _ = button_k3.wait_for_any_edge().fuse() => {
                    (3,button_k3.is_low())
                },
                _ = button_k4.wait_for_any_edge().fuse() => {
                    (4,button_k4.is_low())
                },
                _ = button_k5.wait_for_any_edge().fuse() => {
                    (5,button_k5.is_low())
                },
                _ = button_k6.wait_for_any_edge().fuse() => {
                    (6,button_k6.is_low())
                },
                _ = button_k7.wait_for_any_edge().fuse() => {
                    (7,button_k7.is_low())
                },
                _ = button_k18.wait_for_any_edge().fuse() => {
                    (18,button_k18.is_low())
                },
            }
        });
        let r = if is_low {
            log::info!("Key {} pressed", key);
            tx.send(ControllerCommand::KeyboardPress(key as u8))
        } else {
            log::info!("Key {} released", key);
            tx.send(ControllerCommand::KeyboardRelease(key as u8))
        };
        if r.is_err() {
            break;
        }
    }

    log::warn!("Key listening task ended");

    Ok(())
}

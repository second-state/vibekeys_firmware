# VibeKeys

English | [中文](docs/README_zh.md)

> An ESP32-S3 Rust firmware that turns a custom keypad into a Bluetooth keyboard with speech-to-text (streaming audio to a Whisper service), an LCD UI, MQTT remote control, and OTA updates.

VibeKeys is a Rust firmware for the **ESP32-S3** that turns a piece of custom hardware — screen, keys, and microphone — into a versatile input device:

- **Bluetooth keyboard (BLE HID)** — custom keys act directly as keyboard input to your phone or computer.
- **Voice input** — press the MIC key and speak; the mic audio is first processed by the [esp-sr](https://github.com/espressif/esp-sr) audio front-end (webrtc noise suppression / auto gain), then streamed as a WAV over HTTP to a Whisper service you configure for recognition; the returned text is "typed" into the host through the Bluetooth keyboard. You can also turn off the built-in ASR and pass the mic through to the host to use the host's own dictation.
- **Remote terminal** — connects to the companion **vibetty** over MQTT, sharing / remotely driving the screen and interaction in real time.

## ⚠️ Upgrade note (0.3.x → 0.4.0)

0.4.0 makes incompatible changes to the WiFi-related parts, so **upgrading from 0.3.x to 0.4.0 cannot be done via OTA** — you must perform a **full USB flash** (use one of the `*_bin` images built below, e.g. `vibekeys.bin`). A full flash erases flash, which **invalidates previous settings (WiFi / MQTT server / ASR, stored in NVS); you'll need to reconfigure after upgrading**.

## Key features

- **Two modes**: `Keyboard` (Bluetooth keyboard + ASR) and `Remote` (MQTT remote).
- **ASR (voice input)**: two trigger styles — PTT (push-to-talk) and Toggle (tap to toggle); recognition is done by an HTTP Whisper service (set `asr_config` in `setup.html`: `uri` / `api_key` / `model`); "prefer built-in ASR" can be toggled in settings.
- **LCD UI**: the SPI display renders the keyboard view / remote view / status; optional I2C OLED (`i2c_oled`).
- **Web provisioning**: in AP/OTA mode, open `setup.html` to configure WiFi, MQTT broker, ASR, MIC mode, etc.; stored in NVS.
- **Dual-partition OTA**: a standalone `ota` firmware serves as the bootloader for online upgrades of the main firmware.
- **SNTP**: queries multiple NTP servers in parallel (for HTTPS certificate validation).

## Operation

A **boot menu** always appears at startup with three entries — **Keyboard**, **Remote**, **Setting**. Move with **NEXT** (forward) and **ESC** (back); confirm with **ACCEPT**.

### Keyboard mode (BLE HID + ASR)

The custom keys act as a Bluetooth keyboard. Default keymap (overridable via keymap config):

| Key | Action |
|---|---|
| ACCEPT | Enter |
| BACKSPACE | Backspace |
| ESC | ESC |
| NEXT | ↓ (Down arrow) |
| SWITCH (YOLO) | Shift + Tab |
| CUSTOM | types `/compact` + Enter |
| MIC | Ctrl + Option (trigger host dictation), **or** voice input when built-in ASR is on |
| Rotary push | types `/` |
| Rotary up / down | mouse wheel up / down |

**Voice input (MIC)**: when "prefer built-in ASR" is on and an ASR service is configured, MIC triggers recognition. Two trigger styles (set MIC mode in `setup.html`): **PTT** — hold to record, release to send; **Toggle** — tap to start/stop. The recognized text is typed through the Bluetooth keyboard.

### Remote mode (MQTT → vibetty)

On entering Remote mode the **session picker opens automatically**, listing the vibetty sessions currently published to the broker (it waits briefly for them to arrive).

> **Press the rotary knob at any time to (re)open the session picker** and switch which session is displayed.

In the **session picker**:

| Key | Action |
|---|---|
| NEXT | move focus to the next session |
| ACCEPT | select the focused session and make it active |
| ESC | close the picker without changing |

While viewing a **remote terminal**:

| Key | Action |
|---|---|
| Rotary up / down | scroll the terminal (local pan, then ScrollUp / ScrollDown) |
| ACCEPT | send Enter |
| ESC | send ESC |
| NEXT | send ↓ |
| BACKSPACE | send Backspace |
| CUSTOM | types `/compact` |
| SWITCH | Shift + Tab |
| Rotary push | open the session picker |
| MIC | local voice input (PTT / Toggle); after recognition, an inline editor lets the rotary move the cursor and ACCEPT submit the text |

### Setting

Entered from the boot menu. Options: **WiFi networks**, **OTA Update**, **Clear config**. Move with **NEXT** (or the rotary in sub-screens), pick/edit with **ACCEPT**, delete with **BACKSPACE**, go back with **ESC**. **OTA Update** reboots into the OTA rescue firmware; **Clear config** wipes NVS and reboots.

## Multiple WiFi (wifi_list)

The device keeps a **list of WiFi credentials** (`wifi_list`) rather than a single network. On boot it scans the surroundings and **connects to the first network in the list that is currently in range** — the list order is the priority order.

The point of a list is **mobility**: so the device can follow you between places — **office → café → home** — and come back online at each one without reconfiguring WiFi on the spot. Configure every network you use once (under **Setting → WiFi networks**, or via `setup.html` during provisioning); after that the device picks the right one automatically, wherever you happen to be.

- List order = priority: the first entry whose SSID is visible in the scan wins.
- Up to **8** credentials are stored in NVS (`MAX_WIFI_CREDS`).
- Every mode and the OTA rescue firmware share the same list and the same priority logic, so an over-the-air update also connects from wherever you are.

## Hardware

ESP32-S3 + PSRAM (octal), SPI LCD, I2S microphone, custom keys, optional I2C OLED.

## Building

Built on [Rust + ESP-IDF](https://github.com/esp-rs), target `xtensa-esp32s3-espidf`. Common commands:

```bash
./build.sh keys_bin      # main firmware single image: vibekeys.bin
./build.sh max2_bin      # max2 hardware variant (--features max2)
./build.sh keys_ota_bin  # image with OTA header, upgradable by the ota firmware
./build.sh ota           # OTA bootloader firmware
```

See `./build.sh` for the full list of targets. Feature flags: `max2` (max2 hardware variant), `i2c_oled` (I2C OLED).

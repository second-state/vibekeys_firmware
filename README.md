# VibeKeys

English | [中文](docs/README_zh.md)

> An ESP32-S3 Rust firmware that turns a custom keypad into a Bluetooth keyboard with speech-to-text (streaming audio to a Whisper service), an LCD UI, MQTT remote control, and OTA updates.

VibeKeys is a Rust firmware for the **ESP32-S3** that turns a piece of custom hardware — screen, keys, and microphone — into a versatile input device:

- **Bluetooth keyboard (BLE HID)** — custom keys act directly as keyboard input to your phone or computer.
- **Voice input** — press the MIC key and speak; the mic audio is first processed by the [esp-sr](https://github.com/espressif/esp-sr) audio front-end (webrtc noise suppression / auto gain), then streamed as a WAV over HTTP to a Whisper service you configure for recognition; the returned text is "typed" into the host through the Bluetooth keyboard. You can also turn off the built-in ASR and pass the mic through to the host to use the host's own dictation.
- **Remote terminal** — connects to the companion **vibetty** over MQTT, sharing / remotely driving the screen and interaction in real time.

## ⚠️ Upgrade note (0.3.x → 0.4.0)

0.4.0 makes **breaking changes** to the partition layout (symmetric 4 MB/4 MB OTA slots) and the WiFi-related parts. **Upgrading from 0.3.x to 0.4.0 cannot be done via OTA** — you must perform a **full USB flash** of the bootloader, partition table, and firmware (use one of the `*_bin` images, e.g. `vibekeys.bin`). This **invalidates previous settings** (WiFi / MQTT server / ASR, stored in NVS); you'll need to reconfigure after upgrading.

## Key features

- **Two modes**: `Keyboard` (Bluetooth keyboard + ASR) and `Remote` (MQTT remote).
- **ASR (voice input)**: two trigger styles — PTT (push-to-talk) and Toggle (tap to toggle); recognition is done by an HTTP Whisper service (set `asr_config` in `setup.html`: `uri` / `api_key` / `model`); "prefer built-in ASR" can be toggled in settings.
- **Dual-format remote screen**: JPEG mode (full-frame images, long buffer for local scroll-back) and text mode (vt100 terminal emulation with ANSI colors, incremental dirty-rect rendering). The firmware auto-detects the format from vibetty's presence announcement.
- **LCD UI**: the SPI display renders the keyboard view / remote view / terminal / status; optional I2C OLED (`i2c_oled`).
- **Web provisioning**: in AP/OTA mode, open `setup.html` to configure WiFi, MQTT broker, ASR, MIC mode, etc.; stored in NVS.
- **Dual-partition OTA**: the firmware writes the new image to the inactive OTA partition and reboots into it. Two update sources: browser upload (HTTP PUT), or **download-latest** directly from GitHub releases.
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

Remote mode connects to the vibetty bridge over MQTT. It does **not** bind to a single session — it subscribes to presence for **all of your sessions at once** (`{user}/+/+/vibetty`, retained), so every vibetty terminal you have running shows up, and you can **switch between them on the fly** without reconnecting.

On entering Remote mode the **session picker opens automatically** (it waits briefly for sessions to arrive).

> **Press the rotary knob at any time to (re)open the picker** and switch which session is displayed.

In the **session picker** each session is one row. Its label color reflects that session's live agent state, and the focused row is highlighted separately:

- **white** label — the session is **working** (agent running);
- **orange** label — the session has **stopped** (idle / waiting);
- **blue** background — the currently focused row.

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

When you scroll past the edge of the local buffer, the device asks vibetty for the previous/next page and pops up a `loading...` hint until the new frame arrives.

**JPEG mode**: a tall image buffer (3 screen heights) lets you pan locally; when you reach the top/bottom the device requests the next page from vibetty.

**Text mode**: a vt100 terminal canvas (3 screen heights on max2, 5 on keys) with incremental dirty-rect rendering. The rotary pans the visible window locally; at the canvas edges it sends `scroll_up` / `scroll_down` to vibetty for older/newer history. Delta frames are throttled (≤10 renders/s) and only the changed cells are flushed, keeping the UI responsive during high-frequency output.

### Setting

Entered from the boot menu. Options: **WiFi networks**, **OTA Update**, **Clear config**. Move with **NEXT** (or the rotary in sub-screens), pick/edit with **ACCEPT**, delete with **BACKSPACE**, go back with **ESC**. **OTA Update** enters OTA mode in-process (same firmware, no rescue reboot): it connects WiFi, starts an HTTP server for browser upload, and offers a **download-latest** button to fetch the newest firmware from GitHub releases. **Clear config** wipes NVS and reboots.

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
./build.sh keys_bin      # main firmware merged image (bootloader + partition + app): vibekeys.bin
./build.sh max2_bin      # max2 hardware variant (--features max2)
./build.sh keys          # OTA image (app only, for OTA upload / download-latest)
./build.sh max2          # max2 OTA image
./build.sh keys_ota_bin  # factory image (app in ota_0 slot): vibekeys.bin
./build.sh max2_ota_bin  # max2 factory image: vibekeys_max2.bin
```

See `./build.sh` for the full list of targets. Feature flags: `max2` (max2 hardware variant), `i2c_oled` (I2C OLED).

> The OTA partition layout is symmetric (`ota_0` / `ota_1`, each 4 MB). The `*_bin` images include the bootloader + partition table for first-time flashing; the bare `keys` / `max2` images are app-only for OTA updates.

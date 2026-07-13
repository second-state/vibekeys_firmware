# VibeKeys

[English](../README.md) | 中文

> An ESP32-S3 Rust firmware that turns a custom keypad into a Bluetooth keyboard with speech-to-text (streaming audio to a Whisper service), an LCD UI, MQTT remote control, and OTA updates.

VibeKeys 是一套运行在 **ESP32-S3** 上的 Rust 固件,把一块带屏幕、按键和麦克风的定制硬件变成一个多功能的输入设备:

- **蓝牙键盘(BLE HID)** —— 自定义按键直接作为键盘输入,发到手机 / 电脑;
- **语音输入** —— 按下 MIC 键说话,麦克风音频先经乐鑫 [esp-sr](https://github.com/espressif/esp-sr) 的音频前端(webrtc 降噪 / 自动增益)处理,再以 WAV 流式 HTTP 上传到你配置的 Whisper 服务做识别,返回的文字通过蓝牙键盘"打"进主机;也可关闭内置 ASR,把麦克风透传给主机、用主机自带的听写;
- **远程终端** —— 经 MQTT 连接配套的 vibetty,把屏幕和交互实时共享 / 远程驱动。

## ⚠️ 升级注意(0.3.x → 0.4.0)

由于 0.4.0 对 WiFi 相关部分做了不兼容改动,**从 0.3.x 升级到 0.4.0 不能使用 OTA**,必须通过 **USB 全量刷写**完成升级(用下面构建出的 `*_bin` 镜像,如 `vibekeys.bin`)。全量刷写会擦除 flash,导致**旧的配置(WiFi / MQTT 服务器 / ASR 等 NVS 中的设置)失效,升级后需要重新配置**。

## 主要特性

- **两种工作模式**:`Keyboard`(蓝牙键盘 + ASR)与 `Remote`(MQTT 远程)。
- **ASR(语音输入)**:PTT(按住说话)/ Toggle(点按开关)两种触发方式;识别走 HTTP Whisper 服务(在 `setup.html` 配 `asr_config`:`uri` / `api_key` / `model`),可在设置里开关「优先内置 ASR」。
- **LCD UI**:SPI 屏渲染键盘视图 / 远程视图 / 状态提示;可选 I2C OLED(`i2c_oled`)。
- **Web 配网**:AP/OTA 模式下访问 `setup.html`,配置 WiFi、MQTT broker、ASR、MIC 模式等,参数存 NVS。
- **双分区 OTA**:独立 `ota` 固件作为 bootloader,在线升级主固件。
- **SNTP**:并发查询多个 NTP 服务器(用于 HTTPS 证书校验)。

## 硬件

ESP32-S3 + PSRAM(octal)、SPI LCD、I2S 麦克风、自定义按键,可选 I2C OLED。

## 构建

基于 [Rust + ESP-IDF](https://github.com/esp-rs),target 为 `xtensa-esp32s3-espidf`。常用命令:

```bash
./build.sh keys_bin      # 主固件单镜像 vibekeys.bin
./build.sh max2_bin      # max2 硬件变体(--features max2)
./build.sh keys_ota_bin  # 带 OTA 头、可被 ota 固件升级的镜像
./build.sh ota           # OTA bootloader 固件
```

完整目标见 `./build.sh`。Feature flag:`max2`(max2 硬件变体)、`i2c_oled`(I2C OLED)。

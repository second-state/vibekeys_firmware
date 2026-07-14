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

## 操作方式

开机后**总会先出现启动菜单**,三个条目:**Keyboard**、**Remote**、**Setting**。用 **NEXT**(下一个)、**ESC**(上一个)切换,**ACCEPT** 确认进入。

### 键盘模式(BLE HID + ASR)

各键作为蓝牙键盘输入。默认键位(可通过 keymap 配置覆盖):

| 按键 | 动作 |
|---|---|
| ACCEPT | 回车 |
| BACKSPACE | 退格 |
| ESC | ESC |
| NEXT | ↓(下方向键) |
| SWITCH(YOLO) | Shift + Tab |
| CUSTOM | 输入 `/compact` + 回车 |
| MIC | Ctrl + Option(触发主机听写);开启内置 ASR 时为语音输入 |
| 旋钮按下 | 输入 `/` |
| 旋钮上转 / 下转 | 鼠标滚轮上 / 下 |

**语音输入(MIC)**:开启「优先内置 ASR」并配置好 ASR 服务后,MIC 触发识别,两种触发风格(在 `setup.html` 设 MIC 模式):**PTT**——按住录音、松开发送;**Toggle**——点按开始 / 再点停止。识别出的文字通过蓝牙键盘打出。

### 远程模式(MQTT → vibetty)

远程模式经 MQTT 连接 vibetty 桥接。它**不绑定单个会话**——而是一次性订阅**你所有会话**的 presence(`{user}/+/+/vibetty`,retained),所以你每个在跑的 vibetty 终端都会出现,可以**随时在它们之间切换**,无需重连。

进入远程模式后**会自动打开会话列表**(会先等一小一会儿让会话到达)。

> **随时按下旋钮,即可(重新)打开会话列表**,切换当前显示的会话。

**会话列表**里每个会话占一行。标签颜色反映该会话 agent 的实时状态,焦点行另有高亮(二者独立):

- 标签**白色** —— 该会话**正在工作**(agent 运行中);
- 标签**橙色** —— 该会话**已停下**(空闲 / 等待);
- **蓝色**底 —— 当前焦点所在行。

| 按键 | 动作 |
|---|---|
| NEXT | 焦点移到下一个会话 |
| ACCEPT | 选定当前焦点会话并激活 |
| ESC | 不改变地关闭列表 |

正在查看**远程终端**时:

| 按键 | 动作 |
|---|---|
| 旋钮上转 / 下转 | 滚动终端(本地平移,再发 ScrollUp / ScrollDown) |
| ACCEPT | 发送回车 |
| ESC | 发送 ESC |
| NEXT | 发送 ↓ |
| BACKSPACE | 发送退格 |
| CUSTOM | 输入 `/compact` |
| SWITCH | Shift + Tab |
| 旋钮按下 | 打开会话列表 |
| MIC | 本地语音输入(PTT / Toggle);识别后进入内联编辑器,旋钮移动光标,ACCEPT 提交文字 |

### 设置(Setting)

从启动菜单进入。选项:**WiFi networks**、**OTA Update**、**Clear config**。用 **NEXT**(子界面里也可用旋钮)移动,**ACCEPT** 选定 / 编辑,**BACKSPACE** 删除,**ESC** 返回。**OTA Update** 会重启进入 OTA 救援固件;**Clear config** 清空 NVS 并重启。

## 多 WiFi(wifi_list)

设备保存的是**一份 WiFi 凭据列表**(`wifi_list`),而不是单个网络。开机时扫描周围,**连上当前在范围内、且排在列表最靠前的那个网络**——列表的顺序就是优先级。

用列表而不是单个,是为了**移动场景**:让设备能跟着你换地方——**办公室 → 咖啡厅 → 家里**——每到一处都能自动联网,不用在现场重新配 WiFi。把每个常用网络一次性配好(在 **Setting → WiFi networks**,或配网时的 `setup.html`),之后设备会根据所处位置自动挑对的网络连上。

- 列表顺序 = 优先级:扫描结果里出现的、排在最前的 SSID 胜出。
- NVS 中最多存 **8** 组凭据(`MAX_WIFI_CREDS`)。
- 所有模式、以及 OTA 救援固件都用同一份列表、同一套优先级逻辑——所以 OTA 升级时也能从你当前所在的位置联网。

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

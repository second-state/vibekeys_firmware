# Remote 模式是怎么工作的:MQTT 与 Tailscale

English | 中文

> 面向第一次接触这几个概念的读者:解释 Remote 模式的整体链路、MQTT 协议本身,
> 以及 Tailscale 在其中扮演的角色。协议/实现细节以代码为准。

## 一、MQTT 是什么

**MQTT** 是一个极轻量的**发布/订阅**消息协议,专为嵌入式设备设计。一个 broker,
可以同时服务任意多个 client:

```mermaid
flowchart TB
    broker(("MQTT broker"))

    c1["嵌入式设备<br/>(mqtt-client)"]
    c2["嵌入式设备<br/>(mqtt-client)"]
    c3["App<br/>(mqtt-client)"]

    broker <--> c1
    broker <--> c2
    broker <--> c3
```

> 对 broker 来说,连上来的其它东西——vibekeys 也好,vibetty 也好,还是任何
> App——都只是 client,没有谁是特殊的。

核心概念只有三个:

| 概念             | 一句话解释                                               | vibekeys 里的例子                                |
| ---------------- | -------------------------------------------------------- | ------------------------------------------------ |
| **broker(代理)** | 消息中转站,所有消息经它转发。谁都能连上来收/发           | setup.html 里配的 `mqtt://user:pass@host:1883`   |
| **topic(主题)**  | 消息的「地址」,层级式字符串                              | `my-laptop/vibetty` 等层级组合(实际带用户名前缀) |
| **发布/订阅**    | 发消息的人不关心谁收;想收的人向 broker「订阅」某个 topic | 设备订阅 `<user>/+/+/vibetty`(`+` 是通配符)      |

### vibekeys ↔ vibetty 的分工

- **vibetty**(跑在电脑上):把 agent 会话(如 Claude Code 的终端)的输出发布到 MQTT,并订阅设备发来的按键;
- **vibekeys**(设备):订阅会话广播、接收屏幕画面、发送按键/滚轮/确认。

broker 由 vibetty 内嵌提供时,它就跑在你的电脑上——设备和电脑连的是同一个
broker。它也可以是独立部署的公网服务器,两种方式设备都支持——**连上同一个
broker,vibekeys 和 vibetty 就能互相看见,设备和电脑不需要在同一局域网**。

## 二、示例:Remote 模式的两种部署

开机菜单选 **Remote** 后,键盘变成一块「远程终端显示器」。

### 部署一:broker 内嵌在 vibetty 里(默认)

```mermaid
flowchart LR
    device["VibeKeys 设备<br/>(mqtt-client)"]

    subgraph host["你的电脑"]
        style host fill:none
        subgraph vibetty["vibetty"]
            bridge["会话桥接"]
            broker(("MQTT broker<br/>(内嵌)"))
        end
        subgraph vibetty2["另一个 vibetty"]
            bridge2["会话桥接"]
        end
        agents2["agent 会话<br/>(Claude Code 等)"]
        agents["agent 会话<br/>(Claude Code 等)"]
    end

    device <-- "会话广播 / 屏幕画面" --> broker
    device <-- "按键 / 滚动 / 语音" --> broker
    vibetty2 <--> broker
    bridge <--> agents
    bridge2 <--> agents2
```

不需要单独架服务器;设备和电脑在同一局域网即可。同一台电脑上的
其它 vibetty 实例也可以作为 client 连到这个内嵌 broker。

### 部署二:独立的外部 MQTT 服务器

```mermaid
flowchart LR
    device["VibeKeys 设备<br/>(mqtt-client)"]

    subgraph host["你的电脑"]
        style host fill:none
        subgraph vibetty["vibetty (mqtt-client)"]
            bridge["会话桥接"]
        end
        agents["agent 会话<br/>(Claude Code 等)"]
        subgraph vibetty2["另一个 vibetty (mqtt-client)"]
            bridge2["会话桥接"]
        end
        agents2["agent 会话<br/>(Claude Code 等)"]
    end

    broker(("MQTT broker<br/>(公共服务器)"))

    device <-- "会话广播 / 屏幕画面" --> broker
    device <-- "按键 / 滚动 / 语音" --> broker
    vibetty <-- "发布会话 / 推屏幕" --> broker
    vibetty <-- "收按键" --> broker
    vibetty2 <-- "发布会话 / 推屏幕" --> broker
    vibetty2 <-- "收按键" --> broker
    bridge <--> agents
    bridge2 <--> agents2
```

设备和电脑各自连同一个公共 broker,**两边可以在任何网络**——这就是出差在外的场景。

两种部署设备侧完全一样:连上同一个 broker,vibekeys 和 vibetty 就能互相看见;
区别只在 `server_url` 指向哪(内嵌地址 or 公共服务器)。

**数据流向**:下行是设备从 broker 收到的两条流——「会话存活广播」(拼出屏幕上的
会话列表)和「所选会话的终端画面」(vt100 流,增量渲染);
上行是设备发回去的「按键 / 旋钮 / 语音转出的文字」,由 vibetty 注入对应会话。设备连上 broker 后
这两条流就自动跑起来,断线会自动重连。

## 三、Tailscale 是什么

**部署一**的 broker 跟着你的电脑走——人在办公室,家里那份内嵌 broker 就够不着了。
**Tailscale** 就是解决这个问题的:**部署二**用公共服务器则不需要它。

**Tailscale** 是一个基于 WireGuard 的「虚拟局域网」服务:把你名下的所有设备
(电脑、手机、……以及一块 ESP32)组成一个私网(叫 **tailnet**),每台设备分到
一个固定的内网 IP(`100.64.0.0/10` 段,如 `100.101.5.7`)。

- 设备之间**点对点加密直连**(同一局域网时走局域网;跨网时走 DERP 中继服务器);
- 装了 Tailscale 的设备,无论在哪,都能用 100.x 地址互相访问——**就像插在同一台路由器下**。

### vibekeys 为什么需要它

如果你的 MQTT broker跑在**家里/办公室的电脑上** （部署一）,
设备出了门就连不上它。两个解决办法:

1. 把 broker 挂到公网（暴露端口）;
2. 租用 MQTT-broker （emqx 有免费额度）
3. **让设备也加入你的 tailnet**——broker 变成 `100.x.x.x` 地址,设备走加密隧道直连,不用暴露任何端口。

前两种方式一直可用;**0.4.3 起新增了方案 3**——通过 **microlink** 组件
(C 语言 Tailscale 客户端,WireGuard + DERP + STUN),
设备直接在 ESP32 上加入 tailnet。

### vibekeys 的接入流程

1. **生成 auth key**:在 Tailscale Console->Setting->Key->Generate auth key… 生成一个 `tskey-auth-...` 一次性密钥;
2. **配网绑定**:设备进 Keyboard 模式 → setup.html 的 Tailscale 卡片粘贴 key →
   点 **Bind Tailscale**。设备会:存 key 进 NVS → **擦除旧的 tailnet 注册**
   (机器 key / WireGuard key / 缓存的节点信息,避免换 key 后残留旧记录)→
   **自动跳转 Tailscale 页面**开始连接;
3. **Tailscale 页面**:显示连接进度(Registering → Connected),连上后列出
   tailnet 里的所有节点(在线的排前面);旋钮滚动、ESC 退出(退出即拆隧道);
4. **日常使用**:remote 模式入口检测 broker 地址——是 `100.x` 就自动拉起隧道
   再连 MQTT(超时放宽到 90s,首次经 DERP 建链较慢);不是则完全不碰
   Tailscale,零开销。隧道是透明的,连上后 MQTT 层无感知。

### 使用前提

- MQTT broker 那台电脑也要装 Tailscale 并登录同一账号;
- 设备已通过 Bind Tailscale 绑定过 key(首次注册后设备缓存自己的节点身份,
  后续换 key 建议重新 Bind,会自动清掉旧注册);
- 对时正常(TLS 证书校验需要正确的时钟,remote 入口会自动 SNTP)。

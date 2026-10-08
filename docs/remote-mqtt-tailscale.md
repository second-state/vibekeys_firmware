# How Remote Mode Works: MQTT and Tailscale

English | [中文](remote-mqtt-tailscale_zh.md)

> Written for readers meeting these concepts for the first time: how Remote
> mode works end to end, what MQTT is, and the role Tailscale plays.
> Protocol and implementation details live in the code.

## 1. What is MQTT

**MQTT** is an extremely lightweight **publish/subscribe** messaging protocol
designed for embedded devices. One broker can serve any number of clients:

```mermaid
flowchart TB
    broker(("MQTT broker"))

    c1["Embedded device<br/>(mqtt-client)"]
    c2["Embedded device<br/>(mqtt-client)"]
    c3["App<br/>(mqtt-client)"]

    broker <--> c1
    broker <--> c2
    broker <--> c3
```

> To the broker, everything that connects — VibeKeys, vibetty, any App — is
> just another client. None of them is special.

Three core concepts:

| Concept                 | In one sentence                                                                    | Example in VibeKeys                                                              |
| ----------------------- | ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| **Broker**              | A relay; all messages pass through it. Anyone may connect to publish or subscribe  | The `mqtt://user:pass@host:1883` configured in setup.html                        |
| **Topic**               | The "address" of a message; a hierarchical string                                  | Level combinations like `my-laptop/vibetty` (with a username prefix in practice) |
| **Publish / Subscribe** | Publishers don't care who receives; receivers "subscribe" to a topic on the broker | The device subscribes to `<user>/+/+/vibetty` (`+` is a wildcard)                |

### Division of labor: vibekeys ↔ vibetty

- **vibetty** (runs on your computer): publishes agent session output (e.g. a
  Claude Code terminal) to MQTT, and subscribes to the keys sent by the device;
- **vibekeys** (the device): subscribes to session announcements, receives
  screen content, and sends keys/wheel/confirmations.

When the broker is embedded in vibetty, it runs on your computer — device and
computer connect to the same broker. It can also be a standalone public
server; the device supports both — **connect to the same broker and VibeKeys
and vibetty can see each other, no shared LAN required**.

## 2. Example: two Remote-mode deployments

Selecting **Remote** in the boot menu turns the keypad into a "remote
terminal display".

### Deployment 1: broker embedded in vibetty (default)

```mermaid
flowchart LR
    device["VibeKeys device<br/>(mqtt-client)"]

    subgraph host["Your computer"]
        style host fill:none
        subgraph vibetty["vibetty"]
            bridge["Session bridge"]
            broker(("MQTT broker<br/>(embedded)"))
        end
        subgraph vibetty2["Another vibetty"]
            bridge2["Session bridge"]
        end
        agents2["Agent sessions<br/>(Claude Code etc.)"]
        agents["Agent sessions<br/>(Claude Code etc.)"]
    end

    device <-- "session announcements / screen" --> broker
    device <-- "keys / wheel / voice" --> broker
    vibetty2 <--> broker
    bridge <--> agents
    bridge2 <--> agents2
```

No separate server needed; device and computer just need to be on the same
LAN. Other vibetty instances on the same computer can also connect to this
embedded broker as clients.

### Deployment 2: standalone external MQTT server

```mermaid
flowchart LR
    device["VibeKeys device<br/>(mqtt-client)"]

    subgraph host["Your computer"]
        subgraph vibetty["vibetty (mqtt-client)"]
            bridge["Session bridge"]
        end
        agents["Agent sessions<br/>(Claude Code etc.)"]
        subgraph vibetty2["Another vibetty (mqtt-client)"]
            bridge2["Session bridge"]
        end
        agents2["Agent sessions<br/>(Claude Code etc.)"]
    end

    broker(("MQTT broker<br/>(public server)"))

    device <-- "session announcements / screen" --> broker
    device <-- "keys / wheel / voice" --> broker
    vibetty <-- "publish sessions / push screen" --> broker
    vibetty <-- "receive keys" --> broker
    vibetty2 <-- "publish sessions / push screen" --> broker
    vibetty2 <-- "receive keys" --> broker
    bridge <--> agents
    bridge2 <--> agents2
```

Device and computer each connect to the same public broker, so **both can be
on any network** — that's the travel/commute scenario.

**Data flow**: downstream, the device receives two streams from the broker —
"session announcements" (which build the on-screen session list) and "the
selected session's terminal content" (a vt100 stream, incrementally
rendered). Upstream, the device sends "keys / wheel / voice-to-text" back,
and vibetty injects them into the session. Once the device is connected to
the broker these flows just run; disconnects reconnect automatically.

## 3. What is Tailscale

The **deployment 1** broker lives with your computer — step out of the
office and the embedded broker at home is out of reach. **Tailscale** solves
that; with **deployment 2** (public server) you don't need it.

**Tailscale** is a WireGuard-based "virtual LAN" service: it groups all
devices under your account (computers, phones, ... and an ESP32) into a
private network called a **tailnet**, assigning each device a stable
internal IP from `100.64.0.0/10` (e.g. `100.101.5.7`).

- Devices connect **peer-to-peer with encryption** (over the LAN when local;
  through DERP relay servers across networks);
- Any Tailscale device can reach any other via its 100.x address from
  anywhere — **like being plugged into one router**.

### Why VibeKeys needs it

If your MQTT broker runs on a computer at **home/office** (deployment 1),
the device can't reach it once you leave. Three options:

1. Expose the broker to the public internet (open ports);
2. Rent a public MQTT broker (emqx has a free tier);
3. **Join the device to your tailnet** — the broker becomes a `100.x.x.x`
   address and the device reaches it over an encrypted tunnel, with no
   exposed ports.

Options 1 and 2 have always worked; **0.4.3 adds option 3** — via the
**microlink** component (a C Tailscale client — WireGuard + DERP + STUN),
the device joins the tailnet directly on the ESP32.

### How VibeKeys joins

1. **Generate an auth key**: in the Tailscale Console -> Setting -> Key ->
   Generate auth key…, create a one-off `tskey-auth-...` key;
2. **Bind over BLE**: put the device in Keyboard mode → paste the key into
   the Tailscale card in setup.html → hit **Bind Tailscale**. The device
   stores the key in NVS, **wipes its old tailnet registration** (machine
   key / WireGuard key / cached peers, so a new key isn't shadowed by the
   old registration), and **jumps straight to the Tailscale page** to
   connect;
3. **Tailscale page**: shows connection progress (Registering →
   Connected), then lists every node in the tailnet (online ones first);
   the wheel scrolls and ESC exits (which tears the tunnel down);
4. **Everyday use**: the Remote-mode entry checks the broker address — if
   it's `100.x` the tunnel comes up automatically before connecting MQTT
   (timeout widened to 90s; first contact over DERP is slow); otherwise
   Tailscale is never touched, at zero cost. The tunnel is transparent —
   once up, the MQTT layer doesn't notice it.

### Prerequisites

- The computer hosting the MQTT broker must also run Tailscale under the
  same account;
- The device has bound a key via Bind Tailscale (after first registration
  the device caches its node identity; when changing keys, Bind again — old
  registrations are wiped automatically);
- Time is synced (TLS certificate validation needs a correct clock; the
  remote entry performs SNTP automatically).

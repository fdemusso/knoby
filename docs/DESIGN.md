# Knoby design notes

How Knoby works, what was verified against a real Wave Link install, and which decisions were made (and
reversed) along the way. For usage, see the [README](../README.md).

## Behaviour

Two modes, driven by three keys from the keyboard knob:

| Mode | Rotate | Single click | Double click |
|---|---|---|---|
| **Normal** | volume ± on the active channel | mute / unmute the active channel | open the channel picker |
| **Selecting** | move the selection one icon left / right | confirm the channel and close | n/a |

- The single click in Normal mode is **delayed by the double-click window** (250 ms by default). A second
  click inside the window opens the picker and never touches mute, so mute never fires by accident.
- In Selecting mode a click confirms immediately: there is no ambiguity to resolve there.
- The picker closes by itself after 4 s without input and keeps the previous channel.
- The active channel survives restarts (`%APPDATA%\Knoby\active.txt`).
- If Wave Link is not running, Knoby lets every key through, so the system volume keeps working as usual.

### Picker speed limit

Every detent moves the selection by one icon. If the knob is turned faster than `select_min_ms`, the extra
detents are **dropped, not queued**. Queueing would keep the selection moving after the user stops turning.

## Input

A global low-level keyboard hook (`WH_KEYBOARD_LL`) intercepts and swallows three keys:

| Knob action | Key |
|---|---|
| counter-clockwise | `F14` |
| clockwise | `F15` |
| click | `F13` |

The media keys (`VOLUME_UP/DOWN/MUTE`) are also accepted, but on the author's keyboard the media-key
rotation never reached the hook (the mute key did), for a reason that was not investigated. Dedicated
function keys are unambiguous, never trigger the Windows volume OSD, and are the supported setup. Injected
key events are ignored by default so other software cannot drive Knoby by accident.

## Wave Link integration (verified on Wave Link 3.4.0, build 4650)

Wave Link exposes a local JSON-RPC 2.0 WebSocket server. Its protocol is not covered by a public stability
guarantee, so Knoby is written defensively (reconnects, tolerates unknown fields).

- **Discovery**: the port is written to
  `%LOCALAPPDATA%\Packages\Elgato.WaveLink_g54w8ztgkx496\LocalState\ws-info.json` and changes on every Wave
  Link launch, so it is re-read on each (re)connection.
- **Handshake**: no authentication, but an `Origin: streamdeck://` header is required.
- **Noise**: the server streams `levelMeterChanged` continuously; it is ignored. Responses are matched by
  `id`, never by arrival order.

| Method | Use |
|---|---|
| `getChannels` | channel list: `id`, `name`, `type`, `level` (0-1), `isMuted`, `apps[]`, `image.imgData` |
| `setChannel` | write `level` and / or `isMuted` for a channel `id` |

`getChannels` also returns the microphone as a `Hardware` channel; the picker only lists `type ==
"Software"` channels, so a new software channel created in Wave Link shows up automatically. Names, volume,
mute state and the **icons (72x72 PNG, base64)** all come from the API.

### Staying in sync

Wave Link sends a `channelChanged` notification for every change, from any source (its own UI, a Stream
Deck, Knoby itself), carrying only the changed field (`level` or `isMuted`). Knoby keeps a local cache
updated from these notifications, with no polling.

Its own writes echo back, sometimes late. For 700 ms after Knoby sets a level, level notifications for that
channel are ignored; otherwise a late echo would overwrite the cache with a stale value and the next knob
click would start from the wrong base (which looks exactly like "lost clicks").

## Overlay

- A borderless, topmost, click-through, non-activating layered window
  (`WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED`), so it never steals focus from a
  game.
- **Software rendered** into a premultiplied BGRA buffer and presented with `UpdateLayeredWindow`
  (per-pixel alpha). This gives smooth anti-aliased shapes and animation with no GPU or UI framework.
- Wave Link's icons are 72x72 rounded squares in a per-channel colour. The selection ring and glow reuse the
  **selected icon's own colour**, so the palette stays consistent without hard-coding one.
- Animation: the panel slides in from the screen edge nearest to the configured corner, then the icons
  appear one after the other, and the selection ring slides between icons. It only renders while visible.
- Placement uses the monitor work area (the screen minus the taskbar).

## Architecture

```
 keyboard ──► WH_KEYBOARD_LL hook ──► state machine (Normal / Selecting) ──► overlay (layered window)
                    ▲                          │                                 ▲
                    │ same UI thread           ▼ setChannel                      │ channel cache
              Win32 message loop        Wave Link client thread ◄──── channelChanged notifications
                                         (WebSocket, auto-reconnect)
```

- One UI thread owns the hook, the state machine, the overlay and the tray icon; all Win32 callbacks run on
  it, so application state is a single `thread_local` with no locking.
- One background thread owns the WebSocket. It shares the channel cache behind a `Mutex` and notifies the UI
  thread with a posted window message.
- The hook callback does as little as possible: it decides whether a key is ours, updates state and returns.
- Written in Rust with the `windows` crate. Dependencies: `tungstenite` (WebSocket), `serde_json`, `png`,
  `base64`. The config parser is hand-written to avoid pulling in a TOML/serde stack for seven settings.

## Decisions and dead ends

- **Automatic channel selection was dropped.** The first idea was a deterministic rule set (foreground app,
  last app that made noise, a pool to cycle through). Replacing it with an explicit picker (double click, turn,
  click) removed the unpredictable cases and made the tool easier to trust.
- **Velocity-based volume step was tried and removed.** A step growing from 1% to 5% with rotation speed
  felt wrong in practice; a fixed step is now the default and configurable.
- **Picker sensitivity**: moving every two detents was tried to tame an over-sensitive picker, then replaced
  by one icon per detent plus a speed limit, which keeps single steps meaningful.
- **Lost clicks hunt**: an automated test replaying 36 key presses at 160, 70 and 25 ms spacing produced 36
  adjustments and a final Wave Link level identical to the last value Knoby sent, so the software path is
  lossless. A late-echo overwrite and a 30 ms socket read timeout were fixed along the way (see above).

## Known limitations

- Wave Link 3.x only (developed against 3.4.0). Wave Link 2 uses a different protocol.
- The overlay is a topmost, non-activating window: it is designed to show over windowed and borderless games;
  exclusive fullscreen is not guaranteed.
- The executable is not code-signed, so SmartScreen may warn on first launch of a downloaded copy.
- Idle cost is about 0.3% of one core: the Wave Link thread wakes every 5 ms to keep command latency low. A
  fully event-driven design (socket events delivered to the UI message loop) would remove it and one thread,
  at the price of more delicate connection handling. Judged not worth it yet.

## Roadmap ideas

- Code-sign the release and publish checksums with every release.
- Per-channel step sizes and an optional on-screen volume bar in Normal mode.
- Support for more than one knob / extra key bindings in the config file.

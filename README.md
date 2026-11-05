# Knoby

A tiny Windows utility that makes a keyboard volume knob control the volume of an
[Elgato Wave Link](https://www.elgato.com/wave-link) channel instead of the system master volume.

Status: early design / prototype. See [SPEC.md](SPEC.md) for the full design.

## The problem

A custom keyboard (VIA/QMK) knob sends the standard media keys, so it only moves the Windows
master volume. With Wave Link and a Stream Deck handling per-app volume, you want the knob to drive
the *channel* you are actually listening to.

## How it works

- **Normal mode**: knob rotation changes the volume of the active channel, a single click toggles its mute.
- **Double click**: opens a small rectangular overlay in the top-right corner of the screen showing the
  icons of your Wave Link channels. Rotate to move left/right, single click to confirm.
- Each knob detent changes the volume by a fixed step (3% by default).

Channels, names, icons, volume and mute state all come from Wave Link's local WebSocket API, so any
software channel you create in Wave Link shows up automatically.

## Requirements

- Windows 11
- Elgato Wave Link 3.x (developed against 3.4.0, build 4650)
- A knob remapped in VIA to unused keys: counter-clockwise → `F14`, clockwise → `F15`, click → `F13`.
  (Knoby also accepts `VOLUME_UP` / `VOLUME_DOWN` / `VOLUME_MUTE`, but on the author's keyboard the
  media-key rotation never reached the keyboard hook, so dedicated F-keys are the supported setup.)

Set `KNOBY_DEBUG=1` to log every intercepted key to stderr.

## Build and run

```
cargo build --release
```

The release binary (`target/release/knoby.exe`, ~650 KB) runs without a console window. Knoby lives in
the notification area: right-click its icon for the status, **Start with Windows** (a per-user entry in
`HKCU\...\Run`, pointing at the exe you ran it from) and **Quit**. Only one instance runs at a time.
Debug builds (`cargo run`) keep the console for logging.

## Configuration

Settings live in `%APPDATA%\Knoby\config.toml`, created with commented defaults on first run. Use the
tray menu: **Open config file** to edit it, **Reload config** to apply changes without restarting.

| Setting | Default | Meaning |
|---|---|---|
| `volume_step` | `3` | Volume change per knob detent, in percent (1-20) |
| `select_detents` | `2` | Detents needed to move the picker by one icon (1-6) |
| `select_min_ms` | `90` | Minimum time between two picker moves (0-500) |
| `double_click_ms` | `250` | Double-click window; a single click (mute) fires after this delay (100-600) |
| `select_timeout_ms` | `4000` | The picker closes after this long without input (1000-15000) |
| `overlay_corner` | `"top-right"` | `top-right`, `top-left`, `bottom-right` or `bottom-left` |
| `overlay_margin` | `24` | Distance from the screen edge, in pixels (0-200) |

Invalid values are ignored (the default is used) and reported on stderr in debug builds.

## Wave Link API notes

Wave Link exposes a JSON-RPC 2.0 WebSocket on `127.0.0.1`. The port is written to
`%LOCALAPPDATA%\Packages\Elgato.WaveLink_g54w8ztgkx496\LocalState\ws-info.json` and changes on every
launch. No authentication is needed, only an `Origin: streamdeck://` header. The details Knoby relies on
(`getChannels`, `setChannel`, `channelChanged` notifications) are documented in [SPEC.md](SPEC.md).

## License

[MIT](LICENSE) © 2026 Flavio De Musso.

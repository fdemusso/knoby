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
- Rotation is velocity-sensitive: slow turns move 1% per detent, fast turns up to 5%.

Channels, names, icons, volume and mute state all come from Wave Link's local WebSocket API, so any
software channel you create in Wave Link shows up automatically.

## Requirements

- Windows 11
- Elgato Wave Link 3.x (developed against 3.4.0, build 4650)
- A knob that sends `VOLUME_UP` / `VOLUME_DOWN`, plus its click remapped to `F13` in VIA
  (the default `VOLUME_MUTE` also works)

## Build

```
cargo build --release
```

## Wave Link API notes

Wave Link exposes a JSON-RPC 2.0 WebSocket on `127.0.0.1`. The port is written to
`%LOCALAPPDATA%\Packages\Elgato.WaveLink_g54w8ztgkx496\LocalState\ws-info.json` and changes on every
launch. No authentication is needed, only an `Origin: streamdeck://` header. The details Knoby relies on
(`getChannels`, `setChannel`, `channelChanged` notifications) are documented in [SPEC.md](SPEC.md).

## License

TBD.

use std::error::Error;
use std::io::{Cursor, ErrorKind};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, WebSocket};

const PACKAGE: &str = "Elgato.WaveLink_g54w8ztgkx496";
/// After we set a level ourselves, level notifications for that channel are
/// ignored for this long: they are (possibly late) echoes of our own writes
/// and would otherwise overwrite the cache with stale values.
const ECHO_WINDOW: Duration = Duration::from_millis(700);

/// Premultiplied BGRA pixels, ready for `AlphaBlend`.
pub struct Icon {
    pub w: i32,
    pub h: i32,
    pub bgra: Vec<u8>,
    /// Dominant colour (RGB), sampled from the icon's opaque edge.
    pub accent: [u8; 3],
}

pub struct Channel {
    pub id: String,
    pub name: String,
    pub level: f32,
    pub muted: bool,
    pub icon: Option<Icon>,
    /// Last time we pushed a level ourselves, to ignore our own echoes.
    pub local_set: Option<Instant>,
}

#[derive(Default)]
pub struct Shared {
    pub connected: bool,
    pub channels: Vec<Channel>,
}

enum Cmd {
    SetLevel(String, f32),
    SetMuted(String, bool),
}

pub struct Handle {
    pub shared: Arc<Mutex<Shared>>,
    tx: mpsc::Sender<Cmd>,
}

impl Handle {
    pub fn set_level(&self, id: &str, level: f32) {
        let _ = self.tx.send(Cmd::SetLevel(id.to_owned(), level));
    }

    pub fn set_muted(&self, id: &str, muted: bool) {
        let _ = self.tx.send(Cmd::SetMuted(id.to_owned(), muted));
    }
}

/// Starts the background connection thread. `notify` is called whenever the
/// shared state changes.
pub fn spawn(notify: impl Fn() + Send + 'static) -> Handle {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (tx, rx) = mpsc::channel();
    let s = shared.clone();
    thread::spawn(move || {
        loop {
            if let Err(e) = session(&s, &rx, &notify) {
                eprintln!("wave link: {e}");
            }
            {
                let mut g = s.lock().unwrap();
                g.connected = false;
            }
            notify();
            thread::sleep(Duration::from_secs(2));
            while rx.try_recv().is_ok() {}
        }
    });
    Handle { shared, tx }
}

fn session(
    shared: &Mutex<Shared>,
    rx: &mpsc::Receiver<Cmd>,
    notify: &impl Fn(),
) -> Result<(), Box<dyn Error>> {
    let path = format!(
        "{}\\Packages\\{PACKAGE}\\LocalState\\ws-info.json",
        std::env::var("LOCALAPPDATA")?
    );
    let info: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let port = info["port"].as_u64().ok_or("no port in ws-info.json")?;

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    let mut req = format!("ws://127.0.0.1:{port}").into_client_request()?;
    req.headers_mut()
        .insert("Origin", HeaderValue::from_static("streamdeck://"));
    let (mut ws, _) = tungstenite::client(req, stream)?;
    // Short read timeout = low latency for outgoing commands, which are only
    // drained between reads.
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(5)))?;

    send(&mut ws, json!({"jsonrpc":"2.0","id":1,"method":"getChannels"}))?;

    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if handle_message(shared, &t) {
                    notify();
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.into()),
        }
        while let Ok(cmd) = rx.try_recv() {
            let params = match cmd {
                Cmd::SetLevel(id, level) => json!({"id": id, "level": level}),
                Cmd::SetMuted(id, muted) => json!({"id": id, "isMuted": muted}),
            };
            send(&mut ws, json!({"jsonrpc":"2.0","id":2,"method":"setChannel","params":params}))?;
        }
    }
}

fn send(ws: &mut WebSocket<TcpStream>, v: Value) -> Result<(), tungstenite::Error> {
    ws.send(Message::text(v.to_string()))
}

/// Returns true when the shared state changed.
fn handle_message(shared: &Mutex<Shared>, text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let mut g = shared.lock().unwrap();

    if v["id"] == 1 {
        let Some(list) = v["result"]["channels"].as_array() else {
            return false;
        };
        g.channels = list
            .iter()
            .filter(|c| c["type"] == "Software")
            .filter_map(parse_channel)
            .collect();
        g.connected = true;
        return true;
    }

    if v["method"] == "channelChanged" {
        let p = &v["params"];
        let Some(ch) = g.channels.iter_mut().find(|c| p["id"] == c.id.as_str()) else {
            return false;
        };
        if let Some(l) = p["level"].as_f64() {
            let ours = ch.local_set.is_some_and(|t| t.elapsed() < ECHO_WINDOW);
            if !ours {
                ch.level = l as f32;
            }
        }
        if let Some(m) = p["isMuted"].as_bool() {
            ch.muted = m;
        }
        return true;
    }
    false
}

fn parse_channel(c: &Value) -> Option<Channel> {
    Some(Channel {
        id: c["id"].as_str()?.to_owned(),
        name: c["name"].as_str()?.to_owned(),
        level: c["level"].as_f64().unwrap_or(0.0) as f32,
        muted: c["isMuted"].as_bool().unwrap_or(false),
        icon: c["image"]["imgData"].as_str().and_then(decode_icon),
        local_set: None,
    })
}

fn decode_icon(b64: &str) -> Option<Icon> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let mut bgra = Vec::with_capacity(info.buffer_size());
    for px in buf[..info.buffer_size()].as_chunks::<4>().0 {
        let a = px[3] as u32;
        let pm = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        bgra.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let at = ((3.min(h - 1)) * w + w / 2) * 4;
    let accent = [bgra[at + 2], bgra[at + 1], bgra[at]];
    Some(Icon { w: w as i32, h: h as i32, bgra, accent })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a 4x4 RGBA PNG (opaque orange, except a half-transparent red top-left pixel) as base64.
    fn sample_png_b64() -> String {
        let mut rgba = Vec::new();
        for i in 0..16 {
            rgba.extend(if i == 0 { [255, 0, 0, 128] } else { [200, 100, 50, 255] });
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 4, 4);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    #[test]
    fn decode_icon_premultiplies_to_bgra_and_picks_the_accent() {
        let icon = decode_icon(&sample_png_b64()).expect("valid PNG");
        assert_eq!((icon.w, icon.h), (4, 4));
        // Half-transparent red becomes premultiplied BGRA: b=0, g=0, r=128, a=128.
        assert_eq!(&icon.bgra[0..4], &[0, 0, 128, 128]);
        // Opaque orange (200,100,50) is stored as B,G,R,A.
        assert_eq!(&icon.bgra[4..8], &[50, 100, 200, 255]);
        assert_eq!(icon.accent, [200, 100, 50]);
    }

    #[test]
    fn decode_icon_rejects_garbage() {
        assert!(decode_icon("not base64 at all!").is_none());
        assert!(decode_icon("aGVsbG8=").is_none()); // valid base64, not a PNG
    }

    #[test]
    fn channelchanged_updates_cache_but_ignores_recent_local_level_echoes() {
        let shared = Mutex::new(Shared {
            connected: true,
            channels: vec![Channel {
                id: "A".into(),
                name: "Music".into(),
                level: 0.5,
                muted: false,
                icon: None,
                local_set: None,
            }],
        });
        let note = |body: &str| format!(r#"{{"method":"channelChanged","params":{{"id":"A",{body}}}}}"#);

        // External change: accepted.
        assert!(handle_message(&shared, &note(r#""level":0.7"#)));
        assert_eq!(shared.lock().unwrap().channels[0].level, 0.7);

        // Right after our own write, a (late) level echo must not overwrite the cache...
        shared.lock().unwrap().channels[0].local_set = Some(Instant::now());
        handle_message(&shared, &note(r#""level":0.1"#));
        assert_eq!(shared.lock().unwrap().channels[0].level, 0.7);

        // ...but mute changes are always applied.
        handle_message(&shared, &note(r#""isMuted":true"#));
        assert!(shared.lock().unwrap().channels[0].muted);
    }
}

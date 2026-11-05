# Knoby — spec v0.1

Utility ultra leggera per Windows 11. Fa controllare alla manopola della tastiera custom (VIA/QMK) il volume del **canale Wave Link** scelto, invece del volume di sistema.

## Comportamento

Due stati:

| Stato | Rotazione | Click singolo | Doppio click |
|---|---|---|---|
| `Normal` | volume ± sul canale attivo | mute/unmute del canale attivo | apre l'overlay → `Selecting` |
| `Selecting` | scorre le icone (sx/dx) | conferma → canale attivo, chiude overlay → `Normal` | — |

- Timeout di `Selecting` ~4 s senza input: l'overlay si chiude e resta il canale precedente.
- In `Selecting` la rotazione non tocca il volume.
- Il click singolo in `Normal` viene **ritardato di 250 ms**: se entro la finestra non arriva un secondo click, parte il mute/unmute; se arriva, è un doppio click e si apre l'overlay **senza toccare il mute**. Il mute non scatta mai a vuoto.
- In `Selecting` il click singolo conferma subito, senza ritardo (non c'è ambiguità col doppio click).

## Step del volume

**Fisso, 3% (0.03) per scatto** (`VOLUME_STEP`). Un'accelerazione basata sulla velocità di rotazione (1% → 5%) è stata provata e scartata perché dava fastidio nell'uso reale; il codice è nella cronologia git (modulo `accel.rs`).

In modalità overlay servono 2 scatti per spostare la selezione di un'icona, con almeno 90 ms tra due spostamenti.

## Configurazione

Tutte le costanti di comportamento (step, scatti per icona, finestra del doppio click, timeout dell'overlay, angolo e margine) stanno in `%APPDATA%\Knoby\config.toml`, creato con i default alla prima esecuzione. Voci del menu tray: «Open config file» e «Reload config». Formato: sottoinsieme di TOML (`chiave = valore`, commenti `#`), con parser scritto a mano per non aggiungere dipendenze; i valori non validi tornano al default e vengono segnalati su stderr. Elenco e intervalli: vedi README.

## Overlay

- Rettangolo a angoli squadrati, stesse dimensioni delle notifiche dell'overlay NVIDIA, nella stessa zona dello schermo.
- Una riga di icone dei canali, quella selezionata evidenziata, con il nome.
- Finestra layered, topmost, senza focus e click-through: `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_LAYERED`. Funziona sopra giochi in borderless; il fullscreen esclusivo non è garantito.

## Input

- Hook globale `WH_KEYBOARD_LL`.
- Mappatura VIA (verificata sulla tastiera dell'utente): rotazione antioraria → `F14`, oraria → `F15`, click → `F13`. Knoby intercetta e blocca questi tasti.
- I tasti media (`VOLUME_UP/DOWN/MUTE`) sono accettati come alternativa, ma sulla tastiera dell'utente la rotazione non arrivava all'hook (il mute sì): causa non indagata, si usano i tasti F.

## Wave Link (verificato su 3.4.0 build 4650)

- WebSocket `ws://127.0.0.1:<porta>`. La porta è in `%LOCALAPPDATA%\Packages\Elgato.WaveLink_g54w8ztgkx496\LocalState\ws-info.json` (`{"port": N}`) e **cambia a ogni avvio di Wave Link**: va riletta a ogni (ri)connessione.
- Nessuna autenticazione. Serve l'header `Origin: streamdeck://`.
- JSON-RPC 2.0, `interfaceRevision: 2`.
- Il server manda in continuo la notifica `levelMeterChanged`: da ignorare. Le risposte si abbinano per `id`, non per ordine.

| Metodo | Uso |
|---|---|
| `getApplicationInfo` | versione e revisione dell'interfaccia |
| `getChannels` | elenco canali (vedi sotto) |
| `getMixes` | mix (Personal Mix, Sunshine Mix, Mic Route) |
| `getOutputDevices` / `getInputDevices` | dispositivi |
| `setChannel` | scrittura, vedi sotto |

### getChannels

Ogni canale: `id`, `name`, `type` (`Software` | `Hardware`), `level` (0–1), `isMuted`, `mixes[]`, `apps[]`, `effects[]`, `image.imgData` (PNG 72×72 base64).

**Pool di Knoby = canali con `type == "Software"`**, nell'ordine restituito da Wave Link. L'ingresso `Hardware` (microfono) è escluso. Icone e nomi vengono dall'API, non servono estrazioni dagli exe.

Canali attuali:

| Nome | id |
|---|---|
| System | `PCM_OUT_00_V_02_SD2` |
| Games | `PCM_OUT_00_V_14_SD8` |
| Browser | `PCM_OUT_00_V_12_SD7` |
| Musica | `PCM_OUT_00_V_10_SD6` |
| Coms | `PCM_OUT_00_V_04_SD3` |

### setChannel (testato)

```json
{"jsonrpc":"2.0","id":1,"method":"setChannel","params":{"id":"PCM_OUT_00_V_10_SD6","level":0.13}}
{"jsonrpc":"2.0","id":2,"method":"setChannel","params":{"id":"PCM_OUT_00_V_10_SD6","isMuted":true}}
```

Risposta: `{"result":{"id":"...","level":0.13}}` oppure `{"result":{"id":"...","isMuted":true}}`. Confermato in prova su Musica: il mute si sente e si annulla.

### Notifiche (testato)

Wave Link manda `channelChanged` per ogni cambio, **qualunque sia la sorgente** (app Wave Link, Stream Deck, Knoby stesso). Contiene solo il campo cambiato:

```json
{"method":"channelChanged","params":{"id":"PCM_OUT_00_V_10_SD6","name":"Musica","type":"Software","level":0.14}}
{"method":"channelChanged","params":{"id":"PCM_OUT_00_V_10_SD6","name":"Musica","type":"Software","isMuted":true}}
```

- Il fader di Wave Link procede a passi di 0.01 (~16 msg/s); lo Stream Deck a passi di ~0.0185, a raffica (10–25 ms tra un messaggio e l'altro).
- La sorgente non è distinguibile.
- `level` arriva a 0 e risale senza problemi; il mute non modifica `level`.
- Esistono anche `levelMeterChanged` (rumore, da scartare).

La cache locale (`level`, `isMuted` per canale) si aggiorna **solo dalle notifiche**, senza polling. `getChannels` serve all'avvio e dopo ogni riconnessione.

## Architettura

- **Rust + crate `windows`**, un unico exe, senza finestra principale, icona nella tray. Obiettivo: pochi MB di RAM e ~0% CPU in idle.
- Thread/loop: hook tastiera → macchina a stati → client WebSocket verso Wave Link; overlay su thread UI separato.
- Livello da inviare = livello cache del canale ± step (es. 0.02), con clamp a 0–1. Cache riallineata con `getChannels` ad ogni apertura dell'overlay e dopo ogni riconnessione.
- Riconnessione automatica: se Wave Link non c'è o si riavvia, rileggere `ws-info.json` e riprovare. Senza Wave Link, Knoby lascia passare i tasti media (volume di sistema normale).

## Da verificare / aperto

1. ~~Notifiche di cambio canale~~ — chiuso, vedi «Notifiche».
2. Il `level` di un canale agisce sul fader del mixer o su un mix specifico? Il risultato appare corretto; controllare con `mixes[].level`.
3. ~~Singolo click~~ — chiuso: mute ritardato di 250 ms.
4. ~~Step del volume~~ — chiuso: fisso al 3% (l'accelerazione è stata provata e scartata).
5. Posizione e dimensione esatte dell'overlay (da misurare con quello NVIDIA).
6. Rimappatura del click su `F13` in VIA (a cura dell'utente); la finestra di 250 ms va tarata sul feeling reale.
7. Avvio automatico con Windows.

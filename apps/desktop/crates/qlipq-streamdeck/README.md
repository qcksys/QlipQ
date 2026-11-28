# qlipq-streamdeck — QlipQ Live Meters

A Windows plugin for Stream Deck keys and Stream Deck + dials.

The OBS keypad actions select a scene (preview in Studio Mode), toggle recording, pause or resume,
add an automatically named chapter, mute an input, toggle the replay buffer, and save a replay.
Scene and mute keys each have a target picker. Key states follow OBS across decks. The
[Stream Deck guide](../../../website/src/content/guide/stream-deck.md) covers setup.

**OBS Input Meter** (dial) puts live OBS audio levels on the touch strip. Each dial controls one OBS
audio input:

- the strip shows the input name, its volume in dB, live left/right peak meters and a fader bar;
- turning the dial changes the input's volume (0.2 dB per tick by default);
- pressing the dial or tapping the strip mutes and unmutes it.

**OBS Save Replay** (key) saves OBS's replay buffer on a short press. Holding it for half a second
starts or stops the buffer. The key's image shows whether the buffer is running (red, "Buffer on"),
stopped ("Buffer off") or OBS is closed. Stream Deck's checkmark confirms each saved replay, including
saves from other keys or hotkeys, and its warning triangle shows a failed save, for example while
the buffer is stopped (`src/replay.rs`).

**PC Stats** (dial) shows CPU, GPU and memory use on the touch strip, updated once a second while the
dial is on screen. It reads the counters Task Manager uses (`src/stats.rs`):

- CPU: `\Processor Information(_Total)\% Processor Utility`;
- GPU: `\GPU Engine(*)\Utilization Percentage`, summed per engine across processes; the figure is
  the busiest engine on any GPU, as Task Manager shows per GPU;
- memory: in-use RAM (`GlobalMemoryStatusEx`) as a share of the total.

**Set Output Device** (key) makes a device Windows' Default output (the console and multimedia
roles) and its Communications output in one press, through Windows' undocumented
`IPolicyConfig::SetDefaultEndpoint`, which audio-switcher tools also use (`src/audio.rs`). The plugin
draws the key itself: the left half lights up while the device holds the Default role and the right
half while it holds Communications. It listens for Windows' device notifications
(`IMMNotificationClient`), so a change made elsewhere, such as in Discord or Windows' sound
settings, shows straight away.

**QlipQ actions** drive the QlipQ desktop app's editor through its control pipe,
`\\.\pipe\qlipq-control` (`src/qlipq.rs`, `src/state/qlipq_actions.rs`; the server is
`qlipq-desktop/src/control.rs`). They need QlipQ running, grey out with "QlipQ off" while it isn't, and
reconnect every 2 seconds.

- Keys: **Play/Pause**, **Set In** and **Set Out** (each shows the playhead, in or out time),
  **Export** (shows progress along the bottom while exporting; a press then cancels) and **Rescan**
  (shows the number of clips in the queue).
- **Scrub** (dial): turning steps the playhead by frames or seconds; a press plays or pauses. The
  strip shows the clip, the playhead over its length, a position bar and the in/out times.
- **Clip Picker** (dial): turning moves through the queue in QlipQ's current filter and sort order;
  a press opens the clip. It starts on the open clip and follows QlipQ when the selection changes.

## Settings

The dials, Save Replay, Scene, Mute Input and Set Output Device have settings pages in Stream Deck.
The plugin sanitises whatever is saved and falls back to the default for any value that is out of
range (`Settings::from_payload` in `state.rs`, `replay.rs` and `stats.rs`).

| Action          | Settings (default)                                                                                                    |
| --------------- | --------------------------------------------------------------------------------------------------------------------- |
| OBS Input Meter | `input`, `label` (input name), `step` (0.2 dB), `floor` (-60 dB), `falloff` (100 %/s), `press` (`mute` or `none`)      |
| OBS Save Replay | `label` (Save Replay), `holdAction` (`toggle` or `none`), `hold` (0.5 s), `confirm` (`all` or `own`)                  |
| PC Stats        | `rows` (`cpu`, `gpu`, `ram`, `none` × 3), `interval` (1 s), `gpu` ("" = busiest), `cpuColor`, `gpuColor`, `ramColor` |
| Set Output Device | `deviceId` (the Windows MMDevice endpoint id), `label` (from the device name)                                       |
| QlipQ keys      | `label` (the action's name); Export also `whileExporting` (`cancel` or `nothing`)                                      |
| QlipQ Scrub     | `unit` (`frames` or `seconds`), `amount` (1), `press` (`play` or `none`)                                              |
| QlipQ Clip Picker | `wrap` (false)                                                                                                       |

The PC Stats GPU picker gets its names from the adapters DirectX lists under
`HKLM\SOFTWARE\Microsoft\DirectX`, whose `AdapterLuid` values match the GPU counters' instance names.
The dial samples as often as its fastest visible dial wants, and each dial refreshes on its own interval.

The plugin is a native Windows binary. It talks to Stream Deck over the plugin WebSocket
(`src/streamdeck.rs`) and to OBS over obs-websocket v5 (`src/obs.rs`), subscribing to the
high-volume `InputVolumeMeters` event for the levels and the Outputs events for the replay buffer.
It reads the WebSocket port and password from OBS's own
`%APPDATA%\obs-studio\plugin_config\obs-websocket\config.json` at connect time, so nothing is
configured in the plugin, and it reconnects every 3 seconds while OBS is closed.

## Layout

| Path                                            | What                                                         |
| ----------------------------------------------- | ------------------------------------------------------------ |
| `src/`                                          | The plugin binary                                            |
| `com.qcksys.qlipq.meters.sdPlugin/manifest.json` | Stream Deck actions and controller assignments                |
| `com.qcksys.qlipq.meters.sdPlugin/layouts/`      | Touch-strip layouts (meters, stats, QlipQ scrub and picker)  |
| `com.qcksys.qlipq.meters.sdPlugin/imgs/`         | Action icons and the Save Replay key images                  |
| `com.qcksys.qlipq.meters.sdPlugin/pi/`           | Settings pages, one per action, sharing `pi.js` and `pi.css` |
| `com.qcksys.qlipq.meters.sdPlugin/bin/`          | The built binary (gitignored; copied in by the steps below)  |

## Release package

Each QlipQ release attaches `com.qcksys.qlipq.meters.streamDeckPlugin`, built by the
`streamdeck-plugin` job in `.github/workflows/qlipq-desktop-release.yml`: a release build of the
plugin, packed with Elgato's CLI (`streamdeck pack --version <release version>`), which validates the
manifest. The Desktop workflow runs the same validation and a dry-run pack on every pull request.
Opening the file installs the plugin in Stream Deck.

## Build and install from source (Windows)

From the repo root:

```powershell
cargo test --manifest-path apps/desktop/Cargo.toml -p qlipq-streamdeck
cargo build --manifest-path apps/desktop/Cargo.toml --release -p qlipq-streamdeck
$bundle = "apps/desktop/crates/qlipq-streamdeck/com.qcksys.qlipq.meters.sdPlugin"
New-Item -ItemType Directory -Force "$bundle/bin" | Out-Null
Copy-Item apps/desktop/target/release/qlipq-streamdeck.exe "$bundle/bin/" -Force
```

The first time, link the bundle into Stream Deck's plugin folder and restart Stream Deck:

```powershell
New-Item -ItemType Junction -Path "$env:APPDATA\Elgato\StreamDeck\Plugins\com.qcksys.qlipq.meters.sdPlugin" -Target (Resolve-Path $bundle)
```

To update after a rebuild, stop the running plugin (it locks the `.exe`), copy the new binary in, and
Stream Deck relaunches it within a few seconds:

```powershell
Stop-Process -Name qlipq-streamdeck; Wait-Process -Name qlipq-streamdeck
Copy-Item apps/desktop/target/release/qlipq-streamdeck.exe "$bundle/bin/" -Force
```

Stream Deck reads `manifest.json` only when it starts, so after adding or changing actions, quit
and restart the Stream Deck app instead.

To check the PC Stats counters, or list the output devices and their ids, on this machine:

```powershell
cargo test --manifest-path apps/desktop/Cargo.toml -p qlipq-streamdeck sampler_reads -- --ignored --nocapture
cargo test --manifest-path apps/desktop/Cargo.toml -p qlipq-streamdeck audio_reads -- --ignored --nocapture
```

The audio test only calls the switcher when one device already holds both roles, so it changes nothing.

OBS's WebSocket server must be on (**Tools → WebSocket Server Settings → Enable WebSocket server**).
The plugin logs to `%APPDATA%\Elgato\StreamDeck\logs\com.qcksys.qlipq.meters.log`.

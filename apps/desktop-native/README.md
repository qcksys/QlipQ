# qlipq — cross-platform desktop app (Rust + iced)

A **cross-platform** (Windows / macOS / Linux) build of the qlipq desktop app, written in
Rust with the [iced](https://iced.rs) GUI. It's an alternative front-end to the Windows-only
WinUI 3 app in [`../../desktop`](../../desktop), sharing the same architecture and on-disk data.
It lives in its own Cargo workspace — **not** a member of the Vite+ JS monorepo.

## Architecture

Same split as every qlipq front-end: **the app builds ffmpeg/ffprobe argument strings; the
host only spawns processes.** The two pure crates are Rust ports of the shared TS packages and
are covered by ported parity tests.

| Crate                              | Role                                                                                                                                                                                       |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `crates/qlipq-core`                | Domain model + pure logic — config (+ lenient JSON), edit spec, OBS filename parsing, rename templating, INI/OBS detection, datetimes. Port of `@qcksys/qlipq-core`.                       |
| `crates/qlipq-ffmpeg`              | The single source of truth for ffmpeg/ffprobe **arg building & parsing** — `build_export_args`, `parse_ffprobe`, `parse_progress`, `estimate_export_size`. Port of `@qcksys/qlipq-ffmpeg`. |
| `crates/qlipq-iced` (`bin: qlipq`) | The iced UI + host layer (process spawning, folder scan/watch, config/edits persistence, OBS/NVIDIA detection, frame extraction).                                                          |

The TS packages remain the **parity oracle**; the Rust ports mirror them (and the C# ports) and
the ported `cargo test` suites assert the same behaviour, including exact ffmpeg-arg vectors.

## Build, test & run

Requires a stable Rust toolchain. ffmpeg/ffprobe must be on `PATH` (or set explicit paths in
**Settings → FFmpeg**) — the app shells out to them.

```bash
# From apps/desktop-native/
cargo test -p qlipq-core -p qlipq-ffmpeg   # 72 parity/domain tests
cargo run -p qlipq-iced                     # launch the app
```

Linux build deps (for the GUI crate): `libxkbcommon-dev libwayland-dev libgtk-3-dev`.

## Video preview (deliberate tradeoff)

There is no cross-platform native video widget, and this app avoids linking libav. Instead the
preview **extracts a single frame at the playhead with the ffmpeg CLI** (`-ss … -frames:v 1`,
scaled to ≤720p) and shows it; the scrubber and ±1/5/60s buttons move the playhead and refresh
the frame, and **Play** advances it at a low frame rate. This keeps the build dependency-light
and portable.

As with the other front-ends, **export accuracy comes from ffmpeg `-ss`/`-t`** — the preview is
an advisory guide, not a real-time player. (If real-time A/V playback is needed later, an
`egui-video`-style libav binding or a GStreamer backend could replace the frame extractor.)

## Data compatibility

Config and per-clip edits live in the **same** location and format as the other apps —
`~/.com.qcksys.qlipq/config.json` and `edits.json` (camelCase, with the `$schema` reference) —
and a one-time migration copies them from the old per-OS config dir, so settings and edits carry
over. OBS config and the NVIDIA Share folder are detected per-OS (the NVIDIA registry lookup is
Windows-only, compiled out elsewhere).

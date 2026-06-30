# qlipq — desktop app (Rust)

The qlipq desktop app: a native **Windows-first** build (Linux is also supported; macOS is **not** a
target) written in Rust. It lives in its own Cargo workspace — **not** a member of the
Vite+ JS monorepo. (It supersedes the earlier Tauri and C# / WinUI 3 apps, both since removed.)

## Architecture

Same split as every qlipq front-end: **the app builds ffmpeg/ffprobe argument strings; the
host only spawns processes.** The two pure crates (`qlipq-core`, `qlipq-ffmpeg`) hold the
domain + ffmpeg-arg logic and are covered by unit tests.

| Crate                                 | Role                                                                                                                                                       |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/qlipq-core`                   | Domain model + pure logic — config (+ lenient JSON), edit spec, OBS filename parsing, rename templating, INI/OBS detection, datetimes.                     |
| `crates/qlipq-ffmpeg`                 | The single source of truth for ffmpeg/ffprobe **arg building & parsing** — `build_export_args`, `parse_ffprobe`, `parse_progress`, `estimate_export_size`. |
| `crates/qlipq-desktop` (`bin: qlipq`) | The GUI + host layer (process spawning, folder scan/watch, config/edits persistence, OBS/NVIDIA detection, frame extraction).                              |

The `cargo test` suites assert exact behaviour, including the precise ffmpeg-arg vectors.

## Build, test & run

Requires a stable Rust toolchain. ffmpeg/ffprobe must be on `PATH` (or set explicit paths in
**Settings → FFmpeg**) — the app shells out to them.

```bash
# From apps/desktop/
cargo test -p qlipq-core -p qlipq-ffmpeg          # the crate tests
cargo run -p qlipq-desktop                         # launch the app (in-process libav preview — the default)
cargo run -p qlipq-desktop --no-default-features    # launch with the dependency-light CLI preview
```

Linux build deps (for the GUI crate): `libxkbcommon-dev libwayland-dev libgtk-3-dev`.

## Video preview

There is no cross-platform native video widget, so the preview decodes frames itself and uploads
them to a persistent `wgpu` texture (a custom GPU shader widget — see `src/video.rs`). There are
two backends:

**CLI (`--no-default-features`, dependency-light).** A warm ffmpeg process streams raw RGBA frames to
the app (`src/host.rs`); the scrubber/±buttons move the playhead and **Play** advances at ≤30fps. HDR
is tonemapped to SDR with a CPU zscale chain (an approximation — not VLC-accurate). No audio. This is
the portable build with no native libav dependency — what CI and the release binaries ship.

**`libav-preview` (in-process, VLC-quality) — the default local build.** Decodes with **rsmpeg**
(libav) inside the process: video → **libplacebo** HDR→SDR tonemap (the engine VLC uses — dynamic peak
detection, 203-nit BT.2408 SDR white) and audio → swresample → **cpal**, with audio as the master
clock for A/V sync (`src/libav.rs`). Needs a shared FFmpeg dev build wired via `.cargo/config.toml`; on
by default, so build with `--no-default-features` where that SDK isn't set up (CI / other machines).
_Note: software AV1 decode of very heavy clips (1440p60
10-bit) tops out below realtime, so video lags audio there — hardware-accelerated decode is the
planned fix._

In all cases **export accuracy comes from ffmpeg `-ss`/`-t`** (the unit-tested arg-vector); the
preview is an advisory guide.

## Data compatibility

Config and per-clip edits live in the **same** location and format as the other apps —
`~/.com.qcksys.qlipq/config.json` and `edits.json` (camelCase, with the `$schema` reference) —
and a one-time migration copies them from the old per-OS config dir, so settings and edits carry
over. OBS config and the NVIDIA Share folder are detected per-OS (the NVIDIA registry lookup is
Windows-only, compiled out elsewhere).

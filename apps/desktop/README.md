# qlipq — desktop app (Rust)

The qlipq desktop app: a native **Windows-first** build (Linux is also supported; macOS is **not** a
target) written in Rust. It lives in its own Cargo workspace — **not** a member of the
Vite+ JS monorepo. (It supersedes the earlier Tauri and C# / WinUI 3 apps, both since removed.)

## Architecture

qlipq decodes, previews, probes, and exports **in process** via libav (rsmpeg) — there is no external
`ffmpeg`/`ffprobe` binary. The two pure crates (`qlipq-core`, `qlipq-ffmpeg`) hold the domain +
encode-planning logic and are covered by unit tests.

| Crate                                 | Role                                                                                                                                                                         |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/qlipq-core`                   | Domain model + pure logic — config (+ lenient JSON), edit spec, media info, OBS filename parsing, rename templating, INI/OBS detection, datetimes.                           |
| `crates/qlipq-ffmpeg`                 | Pure encode planning — resolve output settings (`output_settings_to_encode`), the HW encoder + rate-control model (`plan_hw_video`), size estimate (`estimate_export_size`). |
| `crates/qlipq-desktop` (`bin: qlipq`) | The GUI + host layer: in-process libav decode / preview / probe / export, folder scan/watch, config/edits persistence, OBS/NVIDIA detection.                                 |

The `cargo test` suites assert exact behaviour, including the encode-planning + rate-control model.

## Build, test & run

Requires a stable Rust toolchain and a shared **FFmpeg 8.x** dev build wired via the (gitignored)
`apps/desktop/.cargo/config.toml` (`FFMPEG_*` env + the vendored rusty_ffmpeg binding). The app links
libav directly — there is no external-binary path.

```bash
# From apps/desktop/
cargo test -p qlipq-core -p qlipq-ffmpeg     # the pure-crate tests (no FFmpeg link needed)
cargo run -p qlipq-desktop                    # launch the app (in-process libav decode/preview/export)
cargo build --release -p qlipq-desktop        # the shippable binary (CI bundles the FFmpeg shared libs)
```

Linux build deps (for the GUI crate): `libxkbcommon-dev libwayland-dev libgtk-3-dev`.

## Media engine (libav)

There is no cross-platform native video widget, so the preview decodes frames itself and uploads them
to a persistent `wgpu` texture (a custom GPU shader widget — see `src/video.rs`). Everything below runs
**in process** via **rsmpeg** (libav); the app spawns no external process.

**Preview** (`src/libav.rs`) decodes with rsmpeg: video → **libplacebo** HDR→SDR tonemap (the engine
VLC uses — dynamic peak detection, 203-nit BT.2408 SDR white) and audio → swresample → **cpal**, with
audio as the master clock for A/V sync. Preview audio is a monitor mixdown of the enabled tracks.
_Decode is **D3D11VA hardware-accelerated** when the GPU + codec support it (it keeps heavy 1440p10
AV1/HEVC in realtime so video doesn't lag and starve preview audio), with automatic software fallback
on machines without a usable GPU decoder._

**Probe** reads media info + HDR detection straight off the container's codec parameters (`libav::probe`),
replacing the old `ffprobe` shell-out.

**Export** (`src/export.rs`) decodes → applies the edit → hardware-encodes (NVENC/AMF/QSV, planned by
`qlipq_ffmpeg::hw::plan_hw_video`) → muxes, all in process: `export_transcode` when a re-encode is
forced (crop/scale/fps or a non-Original quality), else `export_remux` (lossless video stream-copy;
audio still mixes down via the filtergraph). Enabled audio tracks are summed into one track at the set
levels (matching the preview monitor mix).

## Data compatibility

Config and per-clip edits live in the **same** location and format as the other apps —
`~/.com.qcksys.qlipq/config.json` and `edits.json` (camelCase, with the `$schema` reference) —
and a one-time migration copies them from the old per-OS config dir, so settings and edits carry
over. OBS config and the NVIDIA Share folder are detected per-OS (the NVIDIA registry lookup is
Windows-only, compiled out elsewhere).

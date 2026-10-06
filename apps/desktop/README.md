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

Linux build deps (for the GUI crate): `libxkbcommon-dev libwayland-dev libgtk-3-dev libasound2-dev`.

## Native UI tests

Run from `apps/desktop/` after configuring the FFmpeg SDK:

```bash
cargo test                                      # domain, media, background-work and UI tests
cargo test -p qlipq-desktop --bin qlipq ui_e2e    # native UI workflows only
cargo test -p qlipq-desktop --bin qlipq ui_e2e_hardware_crop_export -- --ignored
```

The UI suite drives the real Iced widget tree with mouse, keyboard, scroll and picker events,
including event subscriptions and asynchronous tasks. It renders through the headless GPU renderer,
generates deterministic two-track recordings with libav, and reads the actual exported media and
persisted files. It covers queue filters/sorting, watching folders, playback and seeking, trim/audio/crop,
tags, rename/delete, settings, shortcuts, highlights, export/cancellation/overwrite, after-export actions,
errors, and the minimum 960 × 660 window. Screenshot checks include preview luminance and the pinned
export controls. The hardware crop test is opt-in because it requires NVENC, AMF or QSV; ordinary tests
use Original video stream-copy with real AAC mixing.

Tests isolate configuration and media in temporary directories. Folder-picker results, OS launch and
clipboard requests, and Ollama responses are controlled at their external boundaries. Native OS dialog
appearance, audible device output, HDR hardware and model accuracy still need device-level validation.
No test launches Explorer, changes the user's settings, or downloads a model.

Screenshots are written to `target/ui-test-artifacts/` and uploaded by the Windows/Linux Desktop CI job.
Headless Linux needs `libvulkan1 mesa-vulkan-drivers` (software Vulkan is sufficient); no display server
is required. See [the UI-thread audit](docs/ui-thread-audit.md) for the execution boundaries and
responsiveness checks.

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

## Highlight suggestions

The editor can suggest an editable trim using a locally installed Ollama vision model (default
`qwen3-vl:4b-instruct`). Start Ollama and run `ollama pull qwen3-vl:4b-instruct`; choose a different installed local
vision model in Settings. No model is bundled or automatically downloaded. The HTTP client connects
only to `127.0.0.1:11434`, bypasses proxies/redirects, and checks that the model is local and supports
vision before sending frames.

`highlight.rs` samples frames with the existing in-process `ScrubDecoder`, then sends timestamped
images to Ollama's `/api/chat` with a response schema derived from the core highlight types. Analysis
uses 30-second windows with 5-second overlap and roughly one sample per second at up to 1280 × 720
to retain HUD detail. The prompt scores execution quality as well as the event: consecutive aimed
headshots should outrank routine multi-kills, and cumulative kill-streak banners are not new kills.
It asks for the complete action sequence. The highest-scoring event becomes a suggested trim with
3 seconds before and 2 seconds after it. Timing validation,
window planning, and padding live in `qlipq-core::highlight`. The user must apply the suggestion;
existing crop/audio edits are preserved. Cancellation or switching clips invalidates pending results.

Requests reserve a 40K-token context and 4,096 output tokens, including any thinking emitted despite
`think: false`. Only the final `message.content` is parsed; `done_reason: "length"` and blank answers
produce actionable errors instead of being treated as invalid JSON or a negative detection.

This initial implementation analyzes visual samples only. Gameplay accuracy has not been benchmarked.
The model can still misread HUD details, misdescribe the event, or choose the wrong range.

To evaluate a labelled recording against the local model and the real detection pipeline, set
`QLIPQ_TEST_INPUT` to its path and `QLIPQ_HIGHLIGHT_EXPECT_START` / `QLIPQ_HIGHLIGHT_EXPECT_END`
to the expected event's first/last seconds, then run
`cargo test -p qlipq-desktop local_highlight_covers_expected_event -- --ignored --nocapture`.
This opt-in test requires Ollama and the default model. It checks that the selected trim contains
the labelled event without passing simply by selecting the whole recording; it does not validate
the model's prose or establish accuracy on other clips.

## Data compatibility

Config and per-clip edits live in the **same** location and format as the other apps —
`~/.com.qcksys.qlipq/config.json` and `edits.json` (camelCase, with the `$schema` reference) —
and a one-time migration copies them from the old per-OS config dir, so settings and edits carry
over. OBS config and the NVIDIA Share folder are detected per-OS (the NVIDIA registry lookup is
Windows-only, compiled out elsewhere).

---
title: Getting started with QlipQ
description: Install QlipQ and export your first clip.
order: 1
---

## 1. Add watched folders

In **Settings → Watched folders**, add the folder(s) where your recordings land (for OBS this is your recording or replay-buffer output path). QlipQ can auto-detect the **OBS** and **NVIDIA Share** output folders and offer them as one-click presets. It scans these folders — including subfolders — on launch and watches for new files while it runs.

## 2. Choose an output folder and naming template

Set an **Output folder** for exports. The **naming template** controls how exported (and renamed) files are named. Available tokens: `{date}`, `{time}`, `{datetime}`, `{source}`, `{name}`, `{index}`.

## 3. Pick your output quality

**Settings → Output defaults** controls export quality and is applied to every export:

- **Quality** — a named preset, a custom **CRF**, **VBR** (CRF capped by a max bitrate), or a **target bitrate**.
- **Frame rate**, **resolution** (down to 720p / up to 4K), **codec** (H.264 / H.265), **container** (mp4 / mkv), and **audio bitrate**.

The editor shows an estimated file size for the current clip, and you can override the quality per clip.

## 4. Edit and export

1. Pick a clip from the **Queue** (each shows its date, length, and size). By default it **starts playing** as soon as it's selected — turn this off with **Settings → Playback → Play clips automatically when selected** to open clips paused.
2. Set the **in/out** points on the timeline. Type a timestamp to jump the playhead, drag the scrubber (playback keeps going if it was already playing), or use the −60/−5/−1 / +1/+5/+60 second jump buttons.
3. **Keyboard shortcuts** default to Adobe Premiere Pro — **Space** play/pause, **I**/**O** set in/out, **←**/**→** step a frame, **Shift+←**/**→** jump 5 s, **Home**/**End** go to start/end, **Ctrl+M** export — and are rebindable in **Settings → Editor shortcuts**.
4. Optionally enable **crop** and adjust the rectangle.
5. Toggle **audio tracks** and set their levels (your selection carries to the next clip); changes are reflected in the preview as you make them. On **export**, the enabled tracks are **mixed together into one track** at the levels you set.
6. Click **Export clip**. If a file with the same name already exists you can **overwrite** it or **append a timestamp** to keep both, and the **After export** setting decides what happens to the original (keep, delete, move, rename, or prompt). Use **Show file** to reveal the exported clip.

> **Preview vs. export.** The preview decodes frames in-process and tonemaps HDR sources to SDR for display — it's a visual guide, and **exports always use the original file, untouched**. Its sharpness is set by **Settings → Preview quality** (720p / 1080p / 1440p / Source; default 1080p) — higher is sharper but costs more decode/GPU work, so lower it if playback stutters. If an HDR clip (especially a Windows HDR _desktop_ recording) previews too dark, **Settings → HDR preview → Brightness** lifts it with an adjustable gamma (higher = brighter; `1.0` = off). Both affect the preview only.

> **Diagnosing preview stutter.** If the preview stutters or the audio drops out, enable **Settings → Playback → Show debug panel in the editor**. The panel adds a **Debug** card under the editor showing the clip's details, whether it's decoding on the **GPU (hardware)** or in **software**, and — while playing — live **video/audio buffer** levels, dropped frames, and audio underruns. Software decoding of heavy 1440p/4K AV1/HEVC is the usual cause; it never affects exports.

**Next:** [set up the OBS replay buffer](/guide/obs-replay-buffer).

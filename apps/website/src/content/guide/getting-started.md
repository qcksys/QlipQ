---
title: Getting started with QlipQ
description: Install QlipQ and export your first clip.
order: 1
---

## 1. Install FFmpeg

QlipQ is powered by [FFmpeg](https://ffmpeg.org) — it shells out to `ffmpeg` and `ffprobe` for probing and exporting. Install FFmpeg and ensure it is available on your `PATH`:

- **Windows:** `winget install Gyan.FFmpeg`
- **macOS:** `brew install ffmpeg`
- **Linux:** `sudo apt install ffmpeg`

If FFmpeg is not on your `PATH`, open QlipQ's **Settings → FFmpeg** and set the full paths to the `ffmpeg` and `ffprobe` binaries. There's a **Test** button next to each to confirm they run.

## 2. Add watched folders

In **Settings → Watched folders**, add the folder(s) where your recordings land (for OBS this is your recording or replay-buffer output path). QlipQ can auto-detect the **OBS** and **NVIDIA Share** output folders and offer them as one-click presets. It scans these folders — including subfolders — on launch and watches for new files while it runs.

## 3. Choose an output folder and naming template

Set an **Output folder** for exports. The **naming template** controls how exported (and renamed) files are named. Available tokens: `{date}`, `{time}`, `{datetime}`, `{source}`, `{name}`, `{index}`.

## 4. Pick your output quality

**Settings → Output defaults** controls export quality and is applied to every export:

- **Quality** — a named preset, a custom **CRF**, **VBR** (CRF capped by a max bitrate), or a **target bitrate**.
- **Frame rate**, **resolution** (down to 720p / up to 4K), **codec** (H.264 / H.265), **container** (mp4 / mkv), and **audio bitrate**.

The editor shows an estimated file size for the current clip, and you can override the quality per clip.

## 5. Edit and export

1. Pick a clip from the **Queue** (each shows its date, length, and size).
2. Set the **in/out** points on the timeline. The player remembers your position and has −60/−5/−1 and +1/+5/+60 second jump buttons.
3. Optionally enable **crop** and adjust the rectangle.
4. Toggle **audio tracks** and set their levels (your selection carries to the next clip).
5. Click **Export clip**. You'll be warned before overwriting an existing file, and the **After export** setting decides what happens to the original (keep, delete, move, rename, or prompt). Use **Show file** to reveal the exported clip.

**Next:** [set up the OBS replay buffer](/guide/obs-replay-buffer).

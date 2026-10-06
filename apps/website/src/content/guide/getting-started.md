---
title: Getting started with QlipQ
description: Install QlipQ and export your first clip.
order: 1
---

## Install

Grab the latest build from [GitHub Releases](https://github.com/qcksys/qlipq/releases/latest):

- **Windows installer** — `qlipq-setup-x64.exe` runs a short setup wizard and adds a Start-menu shortcut and an uninstaller. Recommended.
- **Windows portable** — `qlipq-windows-x64.zip`: no install, just unzip and run `qlipq.exe`.
- **Linux** — `qlipq-linux-x64.tar.gz`: extract and run `qlipq` (loads its bundled FFmpeg libraries from `./lib`).

Every build bundles the FFmpeg libraries QlipQ needs — there's no separate `ffmpeg` to install.

### WinGet (Windows)

Stable releases are submitted to the WinGet community repository. The first package submission must be accepted before later versions can be submitted. Once the package submission is accepted, install or update QlipQ from PowerShell:

```powershell
winget install --id qcksys.qlipq --exact --source winget
winget upgrade --id qcksys.qlipq --exact --source winget
```

WinGet uses the same Windows installer and may request administrator permission. A new release can appear on GitHub before it is accepted into WinGet; if the package or version isn't available yet, use the GitHub installer above.

## Startup options

In **Settings → Library → Startup**, both options are off by default:

- **Start with Windows** (Windows only) — launch QlipQ when you sign in to your Windows account. Turn it off to remove QlipQ's sign-in startup entry. If you move the portable app, launch it from its new location to update the entry.
- **Start minimized** — minimize QlipQ to the taskbar on every launch, including sign-in startup. Restore it from the taskbar when you want to edit; watched folders continue scanning and watching while minimized. Changes apply on the next launch.

These options are independent: enable both to have QlipQ watch for recordings automatically after sign-in with its window minimized.

## 1. Add watched folders

In **Settings → Library → Watched folders**, add the folder(s) where your recordings land (for OBS this is your recording or replay-buffer output path). QlipQ can auto-detect the **OBS** and **NVIDIA Share** output folders and offer them as one-click presets. It scans these folders — including subfolders — on launch and refreshes the queue when recordings are created, changed, moved, renamed, or removed. Updates settle briefly before scanning, so a recording can finish writing before its details are refreshed. **Rescan all folders** also refreshes recordings already in the queue; an unavailable folder keeps its existing entries until it can be scanned again.
Loading, scanning and preview preparation run in the background, so you can continue using the controls while recordings load.

Tick **Hide auto-captured highlights** to keep NVIDIA App's automatic **Highlights** out of the queue (they're tagged `NVIDIA APP (Highlights)` in the file, versus `NVIDIA APP` for a manual recording; a clip's encoder is shown in the editor's debug panel). Highlights that aren't hidden are marked with a **Highlight** badge in the queue. This sets the default; the queue's [filter bar](#4-edit-and-export) can override it per session (show all clips, only highlights, or hide them).

## 2. Choose an output folder and naming template

In **Settings → Export**, set an **Output folder** for exports. QlipQ creates the folder when exporting if it does not exist. The **naming template** controls how exported (and renamed) files are named. Available tokens: `{date}`, `{time}`, `{datetime}`, `{source}`, `{name}`, `{index}`.

## 3. Pick your output quality

**Settings → Export → Output defaults** controls export quality and is applied to every export:

- **Quality** — a named preset, a custom **CRF**, **VBR** (CRF capped by a max bitrate), or a **target bitrate**.
- **Frame rate**, **resolution** (down to 720p / up to 4K), **codec** (H.264 / H.265), **container** (mp4 / mkv), and **audio bitrate**.

The editor shows an approximate file size for the current clip, and you can override the quality per clip. **Target bitrate** gives the most predictable size, but short clips, encoder rate control, and container overhead can still change the result. **Preset**, **CRF**, and **VBR** size estimates are rough: scene detail, motion, and the hardware encoder can make the actual size substantially different. VBR's bitrate is a ceiling, not a target size.

**Original** copies the source video when no crop, resolution, or frame-rate change requires re-encoding. In that case the source codec is preserved. If an edit requires re-encoding, Original uses the same quality level as **High**. Export frame rate and resolution never increase beyond the source. The audio bitrate applies to the single mixed output track, regardless of how many input tracks are enabled.

Use **Output & tags** to override quality per clip. Settings are grouped into **Library**, **Export**, **Preview**, and **Shortcuts** and save automatically.

## 4. Edit and export

Use **Sort** above the clip list to order recordings by **newest/oldest**, **name A–Z/Z–A**, **shortest/longest**, or **largest/smallest**. Newest first is the default and uses the recording date when available, otherwise the file's modified date. Sorting works with the current filters and does not change the selected clip or its edits. The sort choice lasts for the current session; clips whose duration or size is still loading appear after clips with known values.

Each clip has a thumbnail that loads when its card comes into view. **Hover over a clip card** briefly to play a muted preview in its thumbnail; leaving the card restores the still image. Only one hover preview plays at a time, and it loops through the original recording without moving the editor's playhead or changing the selected clip. Clicking the thumbnail opens the clip in the editor as usual.

1. Pick a clip from the **Recordings** list in **Queue** (each shows its date, length, and size). A **filter bar** is pinned above the recordings: search by filename and narrow the list by **status**, **game** (the `{source}` label), **tag**, or **highlights** (all clips / only highlights / hide them). When any filter is active it shows how many clips match and offers **Clear filters** to reset. By default a clip **starts playing** as soon as it's selected — turn this off with **Settings → Preview → Play clips automatically when selected** to open clips paused.
2. In **Trim & highlights**, set the **in/out** points — the timeline highlights the in/out window and marks each endpoint. Playback loops within a trim that ends before the recording's end; reaching the original end stops playback. Each point has an editable timestamp with **±0.5 / ±1 / ±5 s** nudge buttons, or press **Set** (or the **I**/**O** keys) to capture the current playhead. Type a timestamp in the playhead field to jump, drag the scrubber (playback keeps going if it was already playing), or use the −60/−5/−1 / +1/+5/+60 second jump buttons. Timestamps read as frame-accurate timecode — **`h:mm:ss.ff`**, where `ff` is the frame within the second — so a single **←**/**→** step changes the last digits by one.
3. **Keyboard shortcuts** default to Adobe Premiere Pro — **Space** play/pause, **I**/**O** set in/out, **←**/**→** step a frame, **Shift+←**/**→** jump 5 s, **Home**/**End** go to start/end, **Ctrl+M** export — and are rebindable in **Settings → Shortcuts**. Editor shortcuts are inactive in Settings and while typing into a text field. With a clip selected, **Delete** removes its file from disk after a confirmation (**Enter** confirms the prompt) and **Shift+Delete** removes it immediately without asking. **Escape** closes a dialog or exits the fullscreen preview.
4. In **Audio & crop**, optionally enable **crop** and adjust the rectangle.
5. In the same tab, toggle **audio tracks** and set their levels (your selection carries to the next clip); changes are reflected in the preview as you make them. On **export**, the enabled tracks are **mixed together into one track** at the levels you set.
6. Click **Export clip** in the footer, which stays visible as you scroll the editor. It also shows duration, resolution and estimated size. Choose an output folder or filename different from the source recording. If a file with the same name already exists you can **overwrite** it or **append a timestamp** to keep both. One export runs at a time; you can select another clip while it runs and use **Cancel export** in the top bar. The export keeps the settings chosen when it starts, including **Settings → Export → After export** (keep, delete, move, rename, or prompt). That action always applies to the exported clip's original recording. Failed or cancelled exports leave the original in place. Its rename, dismiss and delete actions are disabled until export finishes. Use **Show file** to reveal the exported clip.

The selected recording's card shows **Rename**, **Open**, **Dismiss**, and **Delete**. Dismiss hides a recording without deleting its file; choose the **dismissed** tag filter and use **Restore** to bring it back. Add or remove your own tags in **Output & tags**. If a file operation fails, QlipQ shows the error; a failed rename keeps the dialog open so you can correct the name.

When you rename, move, or delete a recording, its queue entry and saved edits are updated too. QlipQ waits for its preview to close before changing the file. Settings and edits are saved in order, and save failures appear in a dialog.

If no audio output device is available or QlipQ cannot open it, the video preview continues playing silently. Exported audio still follows your enabled tracks and levels.

> **Preview vs. export.** The preview decodes frames in-process and tonemaps HDR sources to SDR for display — it's a visual guide, and **exports use the original media as their input**. Its sharpness is set by **Settings → Preview → Preview quality** (720p / 1080p / 1440p / Source; default 1080p) — higher is sharper but costs more decode/GPU work, so lower it if playback stutters. If an HDR clip (especially a Windows HDR _desktop_ recording) previews too dark, **Settings → Preview → HDR preview → Brightness** lifts it with an adjustable gamma (higher = brighter; `1.0` = off; **Reset** restores the default). Both affect the preview only.

> **Diagnosing preview stutter.** If the preview stutters or the audio drops out, enable **Settings → Preview → Show debug panel in the editor**. The panel adds a **Debug** card under the editor showing the clip's details, whether it's decoding on the **GPU (hardware)** or in **software**, and — while playing — live **video/audio buffer** levels, dropped frames, and audio underruns. Use **Copy** to copy the diagnostics. Software decoding of heavy 1440p/4K AV1/HEVC is the usual cause; it never affects exports.

## Suggest a highlight trim

Highlight suggestions are optional and run through a **local Ollama vision model**. Install [Ollama](https://ollama.com/download), start it, and download the default model:

```sh
ollama pull qwen3-vl:4b-instruct
```

Qwen3-VL requires Ollama 0.12.7 or newer. You can choose another installed local vision model in **Settings → Preview → Highlight suggestions**. QlipQ connects to Ollama at `127.0.0.1:11434`; it does not download models automatically and rejects cloud models.

Use the explicit **`-instruct`** variant. Ollama's `qwen3-vl:4b` tag selects the thinking variant, which can spend the entire output budget reasoning without returning a highlight answer. If you previously configured `qwen3-vl:4b`, download `qwen3-vl:4b-instruct` and select it in **Settings → Preview → Highlight suggestions**. Existing model choices are preserved.

1. Open a clip and find **Highlight suggestion** in **Trim & highlights**, below the In/Out controls.
2. Describe what to find, such as “consecutive headshots”, “a multi-kill”, or “a successful boss fight”, then click **Suggest highlight**. Playback pauses while analysis starts. Longer recordings are analyzed in overlapping sections; the section count shows progress. **Cancel** or selecting another clip stops analysis.
3. Review the suggested range and its description. Your current trim stays unchanged until you click **Apply trim**. **Dismiss** keeps your existing edit.
4. Applying the suggestion adds up to **3 seconds of lead-in** and **2 seconds of aftermath**, bounded by the original clip. A single-instant event, such as a kill notification, gets the same padding. Adjust the normal In/Out controls, preview the result, and export when ready. The applied trim is saved like any other edit.

QlipQ asks the model to compare complete plays and consider execution quality: consecutive aimed headshots should rank above routine or assisted multi-kills. It also asks the model to distinguish new eliminations from persistent kill-streak banners and repeated kill-feed entries. Suggestions aim to include the action sequence, rather than just the notification that follows it. These judgments still depend on the model; it can misread the HUD, misdescribe a play, or choose the wrong moment.

This is an experimental suggestion: analysis samples video at roughly one frame per second, at up to **1280 × 720** to retain HUD detail, in 30-second sections with a 5-second overlap. QlipQ requests a 40K-token context to fit the sampled frames and allow up to 4,096 output tokens, which needs more memory than a short chat. Some models generate thinking text even when asked not to; that uses part of the output budget and can make analysis take longer. QlipQ only uses the final answer. It does not analyze audio and may miss brief action, jokes, or events that require game-specific context. It may report no clear highlight. Accuracy and speed depend on the model, your hardware, and the recording; review every suggested trim.

If Ollama is unavailable or the model is missing, the editor shows an error with setup instructions. An **output limit** error means the model ran out of tokens before finishing its answer; try a shorter clip or a local non-thinking vision model. An **empty answer** error means Ollama finished without returning an answer; retry or choose another local vision model. These errors do not mean that no highlight was found. A failed or cancelled analysis leaves your current trim unchanged.

**Next:** [set up the OBS replay buffer](/guide/obs-replay-buffer).

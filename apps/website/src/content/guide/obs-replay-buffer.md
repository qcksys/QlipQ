---
title: Set up the OBS replay buffer for QlipQ
description: Configure OBS Studio's replay buffer to capture the last few minutes on a hotkey — the ideal source for QlipQ.
order: 2
---

The **replay buffer** keeps the last few minutes of gameplay in memory and writes them to disk only when you press a hotkey. It is the perfect companion to QlipQ: you capture moments as they happen, and QlipQ queues each saved clip for editing.

## 1. Choose an output mode

Open **Settings → Output**. Either _Simple_ or _Advanced_ mode works; Advanced unlocks multiple audio tracks (recommended for QlipQ's per-track mixing).

## 2. Enable the replay buffer

- **Simple mode:** tick `Enable Replay Buffer`, then set `Maximum Replay Time` (e.g. 120s).
- **Advanced mode:** go to the `Replay Buffer` tab and set `Maximum Replay Time`.

> **Format:** choose `mkv` (or fragmented `mp4`) as the recording format. Plain `mp4` can corrupt if OBS crashes mid-record. QlipQ reads `mkv` happily and can remux on export. MKV also preserves OBS's per-track names (Master/Mic/Desktop/…), which `mp4` does not.

## 3. Set the recording path

Under **Settings → Output → Recording**, set the `Recording Path`. This is the folder you will add to QlipQ's **watched folders** (QlipQ can auto-detect it).

## 4. Enable multiple audio tracks (optional but recommended)

In Advanced output mode you can record each audio source to its own track:

1. **Settings → Output → Recording → Audio Track:** tick the tracks you want (e.g. Track 1 = mix, Track 2 = desktop, Track 3 = mic).
2. In the **Audio Mixer**, open the gear → `Advanced Audio Properties` and assign each source to tracks.

QlipQ detects every audio track in the file, so you can mute the mic or rebalance levels per clip at export time.

## 5. Set a filename format QlipQ can read

Go to **Settings → Advanced → Recording → Filename Formatting**. QlipQ parses the timestamp from the name and treats any leading text as the _source_ (game/scene). Good options:

```text
%CCYY-%MM-%DD %hh-%mm-%ss
%CCYY-%MM-%DD_%hh-%mm-%ss
```

Want the game name in the clip too? Prefix it, e.g. `Apex %CCYY-%MM-%DD %hh-%mm-%ss`. QlipQ surfaces it as the `{source}` token in your naming template (and stamps it into the exported clip's metadata).

## 6. Assign hotkeys

Open **Settings → Hotkeys** and bind `Start Replay Buffer`, `Save Replay`, and `Stop Replay Buffer`.

## 7. Run it

1. Start the replay buffer.
2. Play. When something good happens, press **Save Replay**.
3. OBS writes a clip into your recording folder.
4. QlipQ picks it up in the **Queue** — open it, trim, and export.

**Next:** point QlipQ at your recording folder in [Getting started](/guide/getting-started).

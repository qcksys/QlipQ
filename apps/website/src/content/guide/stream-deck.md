---
title: Control OBS from Stream Deck
description: Control scenes, recording, chapter markers, replay buffer and audio from Stream Deck keys. Monitor audio and PC stats on Stream Deck + dials.
order: 4
---

QlipQ Live Meters runs on Windows. Its OBS controls, [QlipQ editor keys](#control-qlipqs-editor) and **Set Output Device** key work on both Stream Deck and Stream Deck +. Stream Deck + also has dials for live audio levels, PC stats, and scrubbing and picking clips in QlipQ. Find the actions under **QlipQ** in the Stream Deck app.

## Install the plugin

The plugin isn't in the release downloads yet. To build and install it from the repository, follow the steps in [`apps/desktop/crates/qlipq-streamdeck/README.md`](https://github.com/qcksys/qlipq/tree/main/apps/desktop/crates/qlipq-streamdeck).

## Switch on OBS's WebSocket server

The OBS actions connect through OBS's built-in WebSocket server.

1. In OBS, open **Tools → WebSocket Server Settings**.
2. Turn on **Enable WebSocket server**. Leave the port and password as they are.
3. Select **OK**.

The plugin reads the port and password from OBS's own settings, so there is nothing to type into Stream Deck. While OBS is closed, the dials and keys show **OBS off**, and the plugin reconnects on its own when OBS starts.

## Set up the OBS keys

Drag a QlipQ action onto a key. If you're moving from Elgato's OBS Studio plugin, you can try the QlipQ actions on a spare page first. Each scene or mute key has its own target.

| QlipQ action        | What pressing the key does                                                                                                                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| OBS Scene           | Selects the configured scene in preview when Studio Mode is on. With Studio Mode off, it selects the live scene. For example, add two keys and choose **Streaming** and **Recording**.                       |
| OBS Record          | Starts or stops recording. The key lights up while recording.                                                                                                                                                |
| OBS Pause Recording | Pauses or resumes the recording. The key lights up while paused.                                                                                                                                             |
| OBS Add Chapter     | Adds the next automatically named chapter to the current recording. A checkmark confirms OBS accepted it. Use a recording format that supports chapters, such as Hybrid MP4, and OBS WebSocket 5.5 or later. |
| OBS Mute Input      | Toggles mute for the selected input. Add a key per input, such as **Mic/Aux** and **Desktop Audio**. A lit key means the input is muted.                                                                     |
| OBS Replay Buffer   | Starts or stops the replay buffer on a single press. A lit key means the buffer is running.                                                                                                                  |
| OBS Save Replay     | Saves the buffer to a clip. For separate **Save** and **Buffer** keys, set **Holding the key** to **Does nothing** on the Save key.                                                                          |

The keys follow changes made in OBS or from another deck. A warning triangle means the command failed, for example when adding a chapter while recording is stopped.

## OBS Input Meter

Each dial controls one OBS audio input, such as **Mic/Aux** or **Desktop Audio**:

- The touch strip above the dial shows the input name, its volume in dB, live left and right level meters, and a fader bar.
- Turn the dial to change the input's volume. Each tick changes it by 0.2 dB by default.
- Press the dial or tap the strip to mute or unmute the input. The strip shows **Muted** in red while it is muted.

To add one:

1. In the Stream Deck app, open the **QlipQ** category and drag **OBS Input Meter** onto a dial.
2. In the settings below the canvas, choose the **OBS input**. The list fills in from OBS while it's running.

Add one dial per input you want on the deck. Each dial also has these settings:

| Setting      | Default        | What it does                                                                       |
| ------------ | -------------- | ---------------------------------------------------------------------------------- |
| Display name | The input name | Text on the strip, e.g. **Mic** for **Mic/Aux**.                                   |
| dB per tick  | 0.2            | How much one click of the dial changes the volume.                                 |
| Floor        | -60 dB         | The quietest level the meter and fader show. Raise it to spread out louder levels. |
| Falloff      | 100 %/s        | How fast the level bars drop after a peak.                                         |
| Press or tap | Mute or unmute | Set to **Nothing** so a press doesn't mute the input by accident.                  |

## OBS Save Replay

Drag **OBS Save Replay** onto a key. It needs OBS's replay buffer, which you set up in **Settings → Output → Replay Buffer**.

- **Press** the key to save the replay buffer as a clip: the last stretch of gameplay, as long as **Maximum Replay Time** in those settings. A checkmark appears when OBS has saved it, including saves from your OBS hotkey or other keys.
- **Hold** the key for half a second to start or stop the replay buffer.

The key shows the buffer's state: red with **Buffer on** while it's running, **Buffer off** while it's stopped, and grey **OBS off** while OBS is closed. A warning triangle means the save didn't happen, usually because the buffer was stopped.

| Setting         | Default                    | What it does                                                                                |
| --------------- | -------------------------- | ------------------------------------------------------------------------------------------- |
| Key label       | Save Replay                | Text on the key, up to 18 characters.                                                       |
| Holding the key | Starts or stops the buffer | Set to **Does nothing** to make every press a save, however long you hold it.               |
| Hold time       | 0.5 s                      | How long to hold before the buffer starts or stops (0.2 to 3 seconds).                      |
| Checkmark       | Every replay save          | **Saves from this key only** skips the checkmark for saves from OBS's hotkey or other keys. |

## PC Stats

Drag **PC Stats** onto a dial to show **CPU**, **GPU** and **RAM** use on the touch strip. The figures match Task Manager: GPU is the busiest part of the graphics card, and RAM is the share of memory in use. Turning or pressing the dial does nothing. PC Stats doesn't need OBS.

| Setting      | Default             | What it does                                                                                                                             |
| ------------ | ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Rows         | CPU, GPU, RAM       | What the top, middle and bottom rows show. Set a row to **Empty** to leave it blank.                                                     |
| Update every | 1 s                 | How often the readings refresh (0.5 to 5 seconds).                                                                                       |
| GPU          | Busiest GPU         | On a PC with more than one graphics card, choose which one to show, e.g. a dedicated card rather than the processor's built-in graphics. |
| Bar colours  | Blue, green, purple | The colour of the CPU, GPU and RAM bars.                                                                                                 |

## Set Output Device

Drag **Set Output Device** onto a key and choose the **Output device** in its settings. Pressing the key makes that device Windows' **Default** output and its **Communications** output, the one apps like Discord use for calls. It doesn't need OBS.

The key shows both roles at once: the left half lights up green while the device is the Default output (**DEF**), and the right half while it's the Communications output (**COM**). Both halves light up when it holds both roles. The key updates straight away when another app or Windows' sound settings change either role, and shows **Not connected** while the device is unplugged. A warning triangle means Windows didn't switch the device.

| Setting       | Default              | What it does                                                                       |
| ------------- | -------------------- | ---------------------------------------------------------------------------------- |
| Output device | None                 | The speakers or headphones the key switches to.                                    |
| Key label     | From the device name | Text on the key, up to 18 characters, e.g. **Headphones** instead of **DX3 Pro+**. |

## Control QlipQ's editor

These actions work the clip open in QlipQ's editor, like its keyboard shortcuts. QlipQ must be running; the keys and dials show **QlipQ off** until it starts and connect on their own when it does. They don't need OBS.

| QlipQ action      | What it does                                                                                                                                                  |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| QlipQ Play/Pause  | Plays or pauses the clip. The key lights up while it plays and shows the playhead time.                                                                       |
| QlipQ Set In      | Sets the trim's in point at the playhead. The key shows the in time.                                                                                          |
| QlipQ Set Out     | Sets the trim's out point at the playhead. The key shows the out time.                                                                                        |
| QlipQ Export      | Exports the clip with your export settings, as **Export clip** does. While it runs, the key shows the progress, and a press cancels the export.               |
| QlipQ Rescan      | Rescans your recording folders. The key shows how many clips are in the queue.                                                                                |
| QlipQ Scrub       | Dial: turn to step the playhead a frame at a time; press or tap to play or pause. The strip shows the clip, the playhead and the in and out times.            |
| QlipQ Clip Picker | Dial: turn to move through the queue, in the filter and sort order QlipQ is showing; press or tap to open the clip. The strip starts on the clip that's open. |

A warning triangle means there was nothing to act on, for example **Set In** with no clip open, or that QlipQ is closed.

| Action      | Setting          | Default            | What it does                                                                 |
| ----------- | ---------------- | ------------------ | ---------------------------------------------------------------------------- |
| QlipQ keys  | Key label        | The action's name  | Text on the key, up to 18 characters.                                        |
| Export      | During an export | A press cancels it | Set to **A press does nothing** so an export can't be cancelled by accident. |
| Scrub       | Unit and amount  | 1 frame            | How far one click moves the playhead, in frames or seconds.                  |
| Scrub       | Press or tap     | Play or pause      | Set to **Nothing** to make the dial scrub only.                              |
| Clip Picker | At the ends      | Stop               | **Wrap around** goes from the last clip back to the first.                   |

# QlipQRenamer

An OBS Studio **Lua** script that auto-sorts finished recordings, saved
replay-buffer clips, and screenshots into **per-game folders** (ShadowPlay-style),
named after the application captured by the scene's **Game Capture** or
**Window Capture** source.

It is based on [the original OBS script](https://obsproject.com/forum/resources/recorder.1926/)
by **oxypatic** (see [Credits](#credits)). The point of this version is to drop the
**Python dependency**: OBS bundles LuaJIT, so a Lua script runs with no separate
Python install or version/path setup.

> **This is the `@qcksys/qlipq-obs-script` package.** `qlipq-renamer.lua` is its
> only artifact (exported as `@qcksys/qlipq-obs-script/qlipq-renamer.lua`). The
> website consumes it as the single source of truth —
> [`qlipq-renamer.lua.ts`](../../apps/website/src/pages/qlipq-renamer.lua.ts) imports
> it with `?raw` and serves it at `/qlipq-renamer.lua` (qlipq.com/qlipq-renamer.lua).
> There is no copied file to keep in sync. Usage is documented in
> [`organize-by-game.md`](../../apps/website/src/content/guide/organize-by-game.md).

## Install

1. OBS → **Tools → Scripts**.
2. **Lua Scripts** tab → **+** → select `qlipq-renamer.lua`.
3. Set the options (see below). Recordings sort automatically from then on.

Requires OBS 28+ (uses `obs_frontend_get_last_*` and the Game Capture
`get_hooked` proc). Works on Windows, macOS, and Linux.

## How it works

- A 1.5 s timer polls the **current scene's visible** capture sources and tracks
  the hooked app's window **title**:
  - **Game Capture** → calls the source's `get_hooked` proc and reads `title`.
  - **Window Capture** → reads the `window` setting (`title:class:executable`)
    and takes the title field.
  - **Neither matched (e.g. Display Capture), Windows only** → reads the OS
    foreground window via LuaJIT FFI (`GetForegroundWindow` + `GetWindowTextA`),
    skipping OBS's own windows and the desktop shell (`explorer.exe`). This is a
    last resort — a real Game/Window Capture always wins.
- On **Recording Stopped**, **Replay Buffer Saved**, and **Screenshot Taken**,
  it takes the last output file and **moves it into a subfolder of its own
  directory**:

  | Mode                       | Recording                  | Replay / Screenshot              |
  | -------------------------- | -------------------------- | -------------------------------- |
  | Folder per game            | `<dir>/<Game>/`            | `<dir>/<Game>/<sub>/`            |
  | Folder per game, then date | `<dir>/<Game>/<yy-mm-dd>/` | `<dir>/<Game>/<sub>/<yy-mm-dd>/` |

  Because the destination is always a subfolder of the file's own directory, the
  move is a same-volume rename (atomic, instant) — no background thread needed.

- The replay path/file can lag the **Replay Buffer Saved** event, so the script
  resolves it (frontend helper, falling back to the replay output's
  `get_last_replay` proc) and processes it on a short one-shot timer.
- The game title is sanitized to `[A-Za-z0-9 ]` (spaces collapsed, trimmed),
  matching the original's behavior.
- When **nothing is detected** (a desktop screenshot, or Display Capture on
  macOS/Linux where the foreground fallback is unavailable), the file goes to the
  **fallback folder** instead.
- Every processed file logs its full naming decision (how the app was detected,
  raw → sanitized title, layout, prefix, result) to OBS's **Script Log**, and
  logs a reason when it can't move a file (no path / file not on disk yet).

## Settings

| Setting                              | Default           | Notes                                                        |
| ------------------------------------ | ----------------- | ------------------------------------------------------------ |
| Fallback folder name                 | `Any Recording`   | Used when no game is detected.                               |
| Organization mode                    | `Folder per game` | Or add a `yy-mm-dd` date subfolder.                          |
| Move clips into per-game folders     | on                | Off = leave files where OBS put them (e.g. tag only).        |
| Prefix filenames with the game title | off               | `Game - original.mkv`.                                       |
| Write game name into file metadata   | off               | Embeds a `game=` tag via ffmpeg (stream copy). See below.    |
| ffmpeg path (for metadata)           | `ffmpeg`          | Full path if ffmpeg isn't on `PATH`. Only used when tagging. |
| Organize replay-buffer saves         | on                |                                                              |
| Replay subfolder name                | _(none)_          | Empty = replays go straight in the game folder.              |
| Organize screenshots                 | on                |                                                              |
| Screenshot subfolder name            | `screenshot`      |                                                              |

"Move into folders" and "Write metadata" are independent: either, both, or
neither. With both off (and no prefix) the file is left untouched.

### Writing the game into metadata

When **Write game name into file metadata** is on, the script runs
`ffmpeg -i <file> -map 0 -c copy -metadata game="<Game>" <out>` — a **stream copy**
(no re-encode), so it's fast and lossless, and writes the same `game=` tag qlipq
stamps on export. Caveats:

- When no game is detected, the tag uses **Fallback folder name** (by default,
  `game=Any Recording`). This also applies with **Move clips into per-game folders** off.
- **AV1 recordings in MKV are supported**: metadata tagging copies the encoded video
  and all audio tracks without decoding or re-encoding them.
- Needs **ffmpeg** available (set the path if it isn't on `PATH`).
- Runs **synchronously** and, on Windows, briefly flashes a console window per clip.
- **mp4** has limited metadata support; **mkv** stores the `game=` tag most reliably.
- The original is never destroyed — ffmpeg writes a temp file first and any failure
  falls back to a plain move (or leaves the file untouched).

The temporary file keeps the container extension (e.g. `clip.qqtmp.mkv`) so ffmpeg
can select the output format. The Script Log reports `tagged + moved` or
`tagged in place` only after success; `moved (untagged)` means no metadata was written.

### Metadata regression tests

From the repository root, with LuaJIT, ffmpeg (including `libaom-av1`), and ffprobe
on `PATH`, run:

```sh
luajit packages/obs-script/tests/metadata.lua
```

The tests invoke the script's OBS event callback with generated AV1/MKV clips and
two AAC tracks. They check default/custom fallback tags, detected titles, tagging
in place (including OBS's forward-slash Windows paths), and preservation of every
encoded packet, plus a failed-ffmpeg fallback.

## Differences from the Python original (intentional)

This is a focused port of the core organizing behavior, not a line-for-line
clone. Deliberately omitted:

- **Live split-file handling.** OBS automatic file splitting produces multiple
  files during one session; the original hooks the recording output's
  `file_changed` signal to move each segment. This version moves the **final**
  segment on `Recording Stopped` only. Earlier segments are left in place.
- **Per-scene source selector.** Instead of selecting and persisting one source
  per scene, this version auto-tracks whichever visible Game/Window Capture source
  in the **current top-level scene** is hooked. Capture sources nested inside
  sub-scenes are not traversed.
- **Date is taken at move time** (`os.date`), not from the file's creation
  timestamp. For a clip that just finished recording these are the same day.
- **No update checker.**

If you need split-file handling or the per-scene selector, they can be added —
both map cleanly onto the OBS C API (the output `file_changed` signal and a
per-scene config table, respectively).

One thing this version **adds** over the original: **Windows foreground-window
detection**, so Display Capture recordings still get sorted (the original only
handled Game/Window Capture). It only kicks in when no capture source identifies a
game, and it skips OBS/Explorer — but note it reads whatever window is focused, so
recording desktop activity with, say, a browser focused will sort under that
window's title rather than the fallback folder.

## Relationship to qlipq

It lives in the monorepo as the `@qcksys/qlipq-obs-script` package, consumed by
the website (which serves it as a download). It is otherwise independent of the
qlipq desktop app — an OBS script, not a build target.

qlipq itself avoids running inside OBS by design, recovering scene/game metadata
after the fact from OBS filename prefixes + `ffprobe`. This script is the opposite
approach: detect the game **live** from inside OBS at capture time. The two
compose — the per-game folders this script creates are exactly what qlipq reads
back as a clip's `{source}`. See
[`organize-by-game.md`](../../apps/website/src/content/guide/organize-by-game.md).

## Credits

QlipQRenamer is an independent **Lua** reimplementation of the idea behind the
original OBS Studio script by **oxypatic**:
<https://obsproject.com/forum/resources/recorder.1926/>

All credit for the original concept and design (per-game file organization)
belongs to its author. See
[Differences from the Python original](#differences-from-the-python-original-intentional)
for what this version changes.

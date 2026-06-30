# RecORDER (Lua)

A lightweight **Lua port** of the [RecORDER](https://obsproject.com/forum/resources/recorder.1926/)
OBS Python script. It auto-sorts finished recordings, saved replay-buffer clips,
and screenshots into **per-game folders** (ShadowPlay-style), named after the
application captured by the scene's **Game Capture** or **Window Capture** source.

The point of the port is to drop the **Python dependency**: OBS bundles LuaJIT,
so a Lua script runs with no separate Python install or version/path setup.

> **This is the `@qcksys/qlipq-obs-script` package.** `recorder.lua` is its only
> artifact (exported as `@qcksys/qlipq-obs-script/recorder.lua`). The website
> consumes it as the single source of truth — [`recorder.lua.ts`](../../apps/website/src/pages/recorder.lua.ts)
> imports it with `?raw` and serves it at `/recorder.lua` (qlipq.com/recorder.lua).
> There is no copied file to keep in sync. Usage is documented in
> [`organize-by-game.md`](../../apps/website/src/content/guide/organize-by-game.md).

## Install

1. OBS → **Tools → Scripts**.
2. **Lua Scripts** tab → **+** → select `recorder.lua`.
3. Set the options (see below). Recordings sort automatically from then on.

Requires OBS 28+ (uses `obs_frontend_get_last_*` and the Game Capture
`get_hooked` proc). Works on Windows, macOS, and Linux.

## How it works

- A 1.5 s timer polls the **current scene's visible** capture sources and tracks
  the hooked app's window **title**:
  - **Game Capture** → calls the source's `get_hooked` proc and reads `title`.
  - **Window Capture** → reads the `window` setting (`title:class:executable`)
    and takes the title field.
- On **Recording Stopped**, **Replay Buffer Saved**, and **Screenshot Taken**,
  it takes the last output file (`obs_frontend_get_last_recording` / `_replay` /
  `_screenshot`) and **moves it into a subfolder of its own directory**:

  | Mode                       | Recording                  | Replay / Screenshot              |
  | -------------------------- | -------------------------- | -------------------------------- |
  | Folder per game            | `<dir>/<Game>/`            | `<dir>/<Game>/<sub>/`            |
  | Folder per game, then date | `<dir>/<Game>/<yy-mm-dd>/` | `<dir>/<Game>/<sub>/<yy-mm-dd>/` |

  Because the destination is always a subfolder of the file's own directory, the
  move is a same-volume rename (atomic, instant) — no background thread needed.

- The game title is sanitized to `[A-Za-z0-9 ]` (spaces collapsed, trimmed),
  matching the original's `__sanitizeTitle`.
- When **nothing is hooked** (e.g. a desktop screenshot, or Display Capture),
  the file goes to the **fallback folder** instead.

## Settings

| Setting                              | Default           | Notes                               |
| ------------------------------------ | ----------------- | ----------------------------------- |
| Fallback folder name                 | `Any Recording`   | Used when no game is detected.      |
| Organization mode                    | `Folder per game` | Or add a `yy-mm-dd` date subfolder. |
| Prefix filenames with the game title | off               | `Game - original.mkv`.              |
| Organize replay-buffer saves         | on                |                                     |
| Replay subfolder name                | `replay`          |                                     |
| Organize screenshots                 | on                |                                     |
| Screenshot subfolder name            | `screenshot`      |                                     |

Recordings are always organized; replays and screenshots are toggleable — same
as the Python original.

## Differences from the Python original (intentional)

This is a focused port of the core organizing behavior, not a line-for-line
clone. Deliberately omitted:

- **Live split-file handling.** OBS automatic file splitting produces multiple
  files during one session; the original hooks the recording output's
  `file_changed` signal to move each segment. This port moves the **final**
  segment on `Recording Stopped` only. Earlier segments are left in place.
- **Per-scene source selector.** Instead of selecting and persisting one source
  per scene, this port auto-tracks whichever visible Game/Window Capture source
  in the **current top-level scene** is hooked. Capture sources nested inside
  sub-scenes are not traversed.
- **Date is taken at move time** (`os.date`), not from the file's creation
  timestamp. For a clip that just finished recording these are the same day.
- **No update checker.**

If you need split-file handling or the per-scene selector, they can be added —
both map cleanly onto the OBS C API (the output `file_changed` signal and a
per-scene config table, respectively).

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

# qlipq

A recording **queue and lightweight clip editor** for gameplay capture. qlipq
watches your capture folders, queues every new recording, and gives you a focused
editor to **trim, crop, pick audio tracks, and export** — all backed by FFmpeg.

> Built as a Vite+ monorepo with a native Rust desktop app, an Astro website,
> and an OBS companion script package.

## Repository layout

```
apps/
  desktop/    Desktop app — Rust (own Cargo workspace, not a pnpm member)
  website/    Astro content site (user guide + download), Cloudflare Workers
packages/
  obs-script/  @qcksys/qlipq-obs-script — QlipQRenamer OBS companion Lua script
```

### What about `plugin-obs`?

The original plan included an OBS plugin to attach metadata to recordings. After
review this was **confirmed unnecessary**: OBS already writes a timestamp (and
optionally a scene/game prefix) into the filename, and qlipq derives the rest
by probing the file in process (libav). The `qlipq-core` crate's `parse_obs_filename` extracts the recorded time and
source label from the name, so no native OBS plugin is required. The website's
[OBS replay buffer guide](apps/website/src/content/guide/obs-replay-buffer.md)
explains the recommended OBS-side setup instead.

## Features

- **Folder watching** — new recordings are added to the queue automatically.
- **Trim** — scrub the timeline and set in/out points (stream-copied for speed).
- **Crop** — pixel-accurate crop rectangle (hardware-encoded via NVENC/AMF/QSV).
- **Audio tracks** — enable/disable and set per-track volume.
- **Renaming** — template-based renaming using the parsed date/source.
- **Export** — in-process libav export with live progress.
- _Future:_ a local AI pass to surface highlights.

## Prerequisites

- [Vite+](https://viteplus.dev) (`vp`) — runtime, package manager, and tooling.
- [FFmpeg](https://ffmpeg.org) **8.x** dev build — only to build/run the desktop app (linked in process via libav; no `ffmpeg`/`ffprobe` on your `PATH`).
- [Rust](https://rustup.rs) — only needed to build/run the desktop app.

## Getting started

```bash
vp install          # install all workspace dependencies
vp check            # format, lint, and type-check
vp run -r build     # build the website
```

### Run the desktop app

The desktop app is a separate Cargo workspace under `apps/desktop/` (Rust).
Needs a Rust toolchain and a shared FFmpeg 8.x dev build wired via the (gitignored) `apps/desktop/.cargo/config.toml` — the app links libav directly (no external `ffmpeg`/`ffprobe`).

```bash
cargo run -p qlipq-desktop                # launch the app (from apps/desktop/; in-process libav decode/preview/export)
cargo test -p qlipq-core -p qlipq-ffmpeg  # the crate tests
cargo build --release -p qlipq-desktop    # produce the shippable `qlipq` binary
```

### Run the website

```bash
vp run qlipq-website#dev        # local dev server
vp run qlipq-website#build      # static build into apps/website/dist
```

## Releasing & CI

- **GitHub Actions** (`.github/workflows/`):
  - `ci.yml` — format, lint, type-check & build the website on every push/PR.
  - `deploy-website.yml` — deploys the site to Cloudflare Workers
    (needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`). Pushes to
    `main` deploy **production** (`qlipq.com`); pushes to `dev` deploy the
    **dev** environment (`dev.qlipq.com`); `workflow_dispatch` lets you pick.
    Locally: `pnpm -C apps/website deploy:dev` / `deploy:prod`.
  - `build-desktop.yml` — builds the Rust desktop app and runs the crate tests
    on Windows and Linux.
  - `release-please.yml` — versions the app and tags it `vX.Y.Z` on merge to
    `main` (crates are internal — not published to crates.io).
  - `qlipq-desktop-release.yml` — on a published `v*` Release, builds and attaches
    the Windows/Linux binaries and the Windows installer, then calls `publish-winget.yml`.
  - `publish-winget.yml` — downloads the published Windows installer, uses
    `wingetcreate update` to update the accepted `qcksys.qlipq` manifests, validates
    them, and submits a PR with `wingetcreate submit` to
    [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Supports the
    first package submission using local templates and subsequent stable `vX.Y.Z` releases; drafts and
    prereleases are rejected. WinGet availability follows Microsoft's PR review
    and merge, not the GitHub release itself.

### WinGet setup and retries

Add the repository Actions secret **`WINGET_TOKEN`** with a GitHub **classic PAT**
with **`public_repo`** scope, as required by
[WingetCreate](https://github.com/microsoft/winget-create/blob/main/doc/token.md).
The token's account submits the PR through its fork of `microsoft/winget-pkgs`;
the repository's `GITHUB_TOKEN` cannot do this. Complete any Microsoft CLA check
requested on the first submission.

To backfill an existing release or retry submission without rebuilding the app,
run **Publish to WinGet** in GitHub Actions with its tag. Leave **submit** off to
validate and download the generated manifests as a workflow artifact; enable it
to submit the PR. The workflow skips submission when that version is already
accepted or has an open PR. While the first package PR is pending, submissions for
other versions also wait; rerun the desired release after the first PR merges.
API failures stop the workflow rather than falling back to a new submission.
Missing credentials fail the submission job; the GitHub release
assets remain available.

WingetCreate is pinned and checksum-verified. Updates start from the latest accepted
upstream manifests, preserving metadata and moderator corrections; local templates
are used only before the first version is accepted. Both paths validate the manifests
and upload them before submission. Installer URLs and release notes use the release tag.

Publication checks, manifest templates, and their generator live in `.github/winget/`. Run
`pwsh -File .github/winget/Test-WinGetManifest.ps1` for local tests, or add
`-ValidateWithWinGet` to also run `winget validate` (requires WinGet). Run
`pwsh -File .github/winget/Test-WinGetPublicationState.ps1` for submission guard tests.
CI runs both test scripts and native validation.

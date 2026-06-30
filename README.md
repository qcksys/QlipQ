# qlipq

A recording **queue and lightweight clip editor** for gameplay capture. qlipq
watches your capture folders, queues every new recording, and gives you a focused
editor to **trim, crop, pick audio tracks, and export** — all backed by FFmpeg.

> Built as a Vite+ monorepo with a Tauri desktop app, an Astro website, and
> shared TypeScript packages.

## Repository layout

```
apps/
  desktop-tauri/   Tauri desktop app — React + TypeScript frontend, Rust backend
  desktop-native/  Cross-platform desktop app — Rust + iced (own Cargo workspace)
  website/         Astro content site (user guide + download), Cloudflare Workers
packages/
  core/       @qcksys/qlipq-core   — domain model, config, OBS filename parsing, renaming
  ffmpeg/     @qcksys/qlipq-ffmpeg — ffmpeg/ffprobe command builders + output parsers
```

### What about `plugin-obs`?

The original plan included an OBS plugin to attach metadata to recordings. After
review this was **confirmed unnecessary**: OBS already writes a timestamp (and
optionally a scene/game prefix) into the filename, and qlipq derives the rest
with `ffprobe`. `@qcksys/qlipq-core`'s `parseObsFilename` extracts the recorded time and
source label from the name, so no native OBS plugin is required. The website's
[OBS replay buffer guide](apps/website/src/pages/guide/obs-replay-buffer.astro)
explains the recommended OBS-side setup instead.

## Features

- **Folder watching** — new recordings are added to the queue automatically.
- **Trim** — scrub the timeline and set in/out points (stream-copied for speed).
- **Crop** — pixel-accurate crop rectangle (re-encodes via libx264).
- **Audio tracks** — enable/disable and set per-track volume.
- **Renaming** — template-based renaming using the parsed date/source.
- **Export** — FFmpeg export with live progress.
- _Future:_ a local AI pass to surface highlights.

## Prerequisites

- [Vite+](https://viteplus.dev) (`vp`) — runtime, package manager, and tooling.
- [FFmpeg](https://ffmpeg.org) on your `PATH` (provides `ffmpeg` + `ffprobe`).
- [Rust](https://rustup.rs) — only needed to build/run the desktop app.

## Getting started

```bash
vp install          # install all workspace dependencies
vp check            # format, lint, and type-check
vp run -r test      # run unit tests in every package
vp run -r build     # build packages, website, and the app frontend
```

### Run the desktop app

```bash
pnpm -C apps/desktop-tauri tauri dev      # dev (hot-reloads the frontend)
pnpm -C apps/desktop-tauri tauri build    # produce installers
```

### Run the website

```bash
vp run qlipq-website#dev        # local dev server
vp run qlipq-website#build      # static build into apps/website/dist
```

## Releasing & CI

- **Changesets** version the shared packages. Record a change with
  `vp run changeset`; merging to `main` opens a "Version Packages" PR, and
  merging that publishes `@qcksys/qlipq-core` and `@qcksys/qlipq-ffmpeg`.
- **GitHub Actions** (`.github/workflows/`):
  - `ci.yml` — check, test, and build on every push/PR.
  - `release.yml` — changesets versioning/publishing (needs `NPM_TOKEN`).
  - `deploy-website.yml` — deploys the site to Cloudflare Workers
    (needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`). Pushes to
    `main` deploy **production** (`qlipq.com`); pushes to `dev` deploy the
    **dev** environment (`dev.qlipq.com`); `workflow_dispatch` lets you pick.
    Locally: `pnpm -C apps/website deploy:dev` / `deploy:prod`.
  - `build-app.yml` — builds the desktop app on Windows/macOS/Linux and
    attaches installers to a GitHub release when a `v*` tag is pushed.

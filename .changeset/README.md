# Changesets

This folder is managed by [changesets](https://github.com/changesets/changesets).

To record a change for the next release, run:

```bash
vp run changeset
```

Pick the affected packages and a semver bump, then describe the change. The
generated markdown file is committed alongside your PR. On merge to `main`, the
release workflow opens a "Version Packages" PR; merging that publishes the
public packages (`@qlipq/core`, `@qlipq/ffmpeg`).

The desktop app and website are versioned via release tags, not changesets, so
they are listed under `ignore` in `config.json`.

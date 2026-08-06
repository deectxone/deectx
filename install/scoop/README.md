# Scoop bucket setup

`deectx.json` here is the source of truth for the Scoop manifest; the
`release` workflow refreshes its hash/URL on every tag and copies it into the
[deectxone/scoop-deectx](https://github.com/deectxone/scoop-deectx) bucket
repo, which is what `scoop install deectx` actually reads from (Scoop
requires manifests at a bucket repo's root — it won't look inside a
subfolder of the main deectx repo).

## One-time setup (already done if this note is gone)

1. Create the `deectxone/scoop-deectx` repo with a `bucket/` folder
   containing this manifest (see the repo's own README for the exact
   layout).
2. Generate a fine-grained GitHub PAT with **Contents: write** access scoped
   to `deectxone/scoop-deectx` only.
3. Add it as a repository secret named `SCOOP_BUCKET_TOKEN` in the `deectx`
   repo (Settings → Secrets and variables → Actions).

Once that secret exists, every tagged release automatically pushes the
refreshed manifest to the bucket. Until then, the release workflow logs a
notice and skips that step — Homebrew and the GitHub Release artifacts are
unaffected.

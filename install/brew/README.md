# Homebrew tap setup

`deectx.rb` here is the source of truth for the Homebrew formula; the
`release` workflow refreshes its version/URLs/hashes on every tag and copies
it into the
[deectxone/homebrew-deectx](https://github.com/deectxone/homebrew-deectx)
tap repo, which is what `brew tap deectxone/deectx && brew install deectx`
actually reads from (Homebrew taps require the formula at `Formula/<name>.rb`
in a repo named `homebrew-<tap>` — it won't look inside a subfolder of the
main deectx repo).

## One-time setup (already done if this note is gone)

1. Create the `deectxone/homebrew-deectx` repo with a `Formula/deectx.rb`
   file (this file, copied in).
2. Generate a fine-grained GitHub PAT with **Contents: write** access scoped
   to `deectxone/homebrew-deectx` only.
3. Add it as a repository secret named `HOMEBREW_TAP_TOKEN` in the `deectx`
   repo (Settings → Secrets and variables → Actions).

Once that secret exists, every tagged release automatically pushes the
refreshed formula to the tap. Until then, the release workflow logs a notice
and skips that step — Scoop and the GitHub Release artifacts are unaffected.

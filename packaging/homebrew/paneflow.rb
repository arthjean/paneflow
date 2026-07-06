# PaneFlow Homebrew cask - canonical template.
#
# US-017. This file is the SOURCE OF TRUTH that
# `.github/workflows/update_cask.yml` copies (and version-stamps) into
# the external tap repo `ArthurDEV44/homebrew-paneflow` on every
# release. The tap repo's `Casks/paneflow.rb` is a derived artifact; do
# not hand-edit it - commit changes here instead and let the workflow
# propagate on the next release.
#
# Operator setup (one-time):
#   1. Create the public repo https://github.com/ArthurDEV44/homebrew-paneflow
#   2. Initialise it with an empty `Casks/` directory (`git init && mkdir
#      Casks && git commit -m "bootstrap"`).
#   3. Create an ed25519 SSH deploy key on the tap repo with write access,
#      add the private half to this repo's Actions secrets as
#      `HOMEBREW_TAP_DEPLOY_KEY`.
#   4. The next release (update_cask.yml chains off the `release`
#      workflow's completion via workflow_run) will render + push the
#      tap's `Casks/paneflow.rb`. The tap was bootstrapped manually for
#      v0.3.4 on 2026-05-28; the workflow keeps it current thereafter.

cask "paneflow" do
  # These two lines are rewritten on every release by the CI workflow.
  # The placeholders keep the file syntactically valid (so `brew style`
  # passes in CI) and flag that a human-edited version is stale.
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/ArthurDEV44/paneflow/releases/download/v#{version}/paneflow-#{version}-aarch64-apple-darwin.dmg",
      verified: "github.com/ArthurDEV44/paneflow/"

  name "PaneFlow"
  desc "Native terminal workspace for parallel coding agents"
  homepage "https://paneflow.dev"

  # `ventura` (macOS 13) is the floor because GPUI's macOS backend targets
  # that era. Same value as `LSMinimumSystemVersion` in assets/Info.plist
  # (US-013). Bumping this here without bumping the plist (or vice versa)
  # causes install-time confusion - keep them synchronised.
  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  # Gatekeeper on macOS extracts the bundle from the DMG and installs it;
  # Homebrew handles mount/copy/unmount around this `app` stanza.
  app "PaneFlow.app"

  # `zap trash:` is Homebrew's opt-in deep-clean; `brew uninstall --zap`
  # moves these directories to the user's Trash. The paths match what
  # PaneFlow writes at runtime:
  #   ~/Library/Application Support/paneflow   → session.json, config.json
  #   ~/Library/Caches/paneflow                → scrollback, update-check cache
  # We intentionally do NOT zap ~/Library/Preferences/* - those may hold
  # Apple-system-managed state (window sizes, traffic-light geometry)
  # that shouldn't be nuked on an uninstall.
  zap trash: [
    "~/Library/Application Support/paneflow",
    "~/Library/Caches/paneflow",
  ]
end

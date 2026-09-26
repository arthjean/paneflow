---
name: release
description: Ship a Paneflow release end to end, from version bump to green CI, published notes, package streams, and paneflow.dev.
disable-model-invocation: true
argument-hint: "[version, e.g. 0.17.0]"
---

# Release Paneflow $ARGUMENTS

Take `$ARGUMENTS` from a clean `main` to a release every user can install, and run until the contract below is fully green. Arthur invokes this and walks away: the run is his hands on the keyboard for the whole release.

## Mandate

Invoking `/release` authorizes, for this version only: the bump commit and its push to `main`, the annotated tag and its push, CI reruns and `workflow_dispatch` recovery runs, source fixes committed to `main` when a gate or leg is red, editing the GitHub Release title and body, and the release-sync commit pushed to `paneflow-web` `main` (which deploys paneflow.dev). The site's `AGENTS.md` line "Preparation does not authorize publication" is satisfied by this invocation. Anything Arthur writes alongside the version overrides this skill.

Keep going between steps. Put status in one line beside the next tool call. A summary that names the next step, an offer to continue, and a menu of options are all still mid-run while any contract row is unmet. Stop only for one of the four blockers:

1. A credential or external service you cannot restore yourself (secret, runner outage, GitHub or Vercel down past a retry).
2. A source fix needed after the GitHub Release is published: the next version number is Arthur's call. Prepare and gate the fix first, so his answer is the only missing input.
3. A destructive action beyond the one re-tag case in [recovery](references/red.md): deleting a published release, force-pushing, rewriting pushed history.
4. The version collides with an existing tag or does not advance the channel.

If a line of this skill is what makes you pause, quote it in the report and say whether it is an explicit requirement or your reading of it.

## Done means

| Channel | Every row must hold, each with evidence |
|---|---|
| Stable `X.Y.Z` | Tag-push `release` run succeeded; the GitHub Release is published, marked latest, carries the full signed asset set and the final notes; `repo_publish` and `update_cask` each ran their job (not skipped) for this tag and succeeded; apt metadata and the cask report `X.Y.Z`; the Ubuntu and Fedora container installs print `paneflow X.Y.Z`; paneflow.dev serves `X.Y.Z` from a ready deployment of the release-sync commit, and its post-deploy check passes. |
| `-rc.N`, `-alpha.N`, `-beta.N` | Tag-push `release` run succeeded; the GitHub Release is a published prerelease with the full signed asset set and the final notes. Streams, cask, and site stay on the last stable. |

A red, skipped, or unverified row is an open row. Keep the run going until the contract holds or one of the four blockers applies.

## Ledger

Keep the run's state in `tasks/release/vX.Y.Z/` (local, gitignored): `state.md` is a checklist of the steps below plus the facts as they land (previous tag, release commit SHA, run IDs and URLs, asset count, site commit, deployment URL); `notes.md` is the release notes. Tick each item as its criterion holds. The ledger survives context compaction and a second session: on start, if `state.md` exists, resume from the first unticked item after re-verifying the last ticked one against live state.

Commands live in [the runbook](../../../docs/release/runbook.md), Steps 1 to 6. `AGENTS.md` owns the gates and their exact flags. When the runbook and a live workflow disagree, trust the workflow, and name the discrepancy in the report.

## Steps

1. **Preflight.** Strip a leading `v` and validate the version against the runbook's regex. `git fetch --tags origin`, then confirm `main` is not behind `origin/main` and that `vX.Y.Z` exists neither locally nor on the remote (`git ls-remote --tags origin`). Local commits ahead of `origin/main` are Arthur's finished work and ship with this release; list them in `state.md`. Uncommitted changes are Arthur's work in progress: leave them untouched and release from a worktree of `main` (`git worktree add <scratch>/paneflow-release main`, `CARGO_TARGET_DIR` pointed at this checkout's `target/`). Set the previous tag: the last stable tag reachable from `main` for a stable release, so prerelease changes stay covered; the last tag of any kind for a prerelease. *Criterion:* version, channel, previous tag, and release base SHA are written in `state.md`.

2. **Ledger and notes.** The ledger is a pixel-exact account of every change since the previous tag, built by subagents and reviewed by you.

   *Fan out.* List the range with `git rev-list --reverse --no-merges <previous>..HEAD` and cut it into contiguous slices of about ten commits, a commit with a very large diff taking a slice alone. Give each slice to its own `general-purpose` subagent, all launched in one message, with this brief: read the full diff of every commit in the slice (`git show --stat --patch <sha>`), not its message alone, and return one entry per SHA with: user-facing or internal; what changed for the user, down to exact values (sizes in px, colors, durations, labels, shortcuts, menu paths, defaults); every added, changed, or removed config key, CLI verb, IPC or MCP method, file path, and platform-specific branch; anything breaking or needing migration; and `path:line` evidence for each claim. Tell it the report is read by a human and must list every SHA of its slice.

   *Review each report as it arrives, before accepting it.* Check that its SHA list equals its slice exactly. Open the cited hunk (`git show <sha> -- <path>`) for every user-facing claim and confirm the value and behavior match the diff; spot-check the commits it marked internal. A report with a missing SHA, a claim the diff does not support, or a vague entry ("improves the UI") goes back to the same subagent through `SendMessage` with the precise gap, and is reviewed again on return. Mark each slice accepted in `state.md` only then. Once all slices are accepted, check that the union of their SHAs equals the range, and reconcile the result with `CHANGELOG.md` `[Unreleased]`: an entry there with no commit behind it, or a user-facing commit missing from it, is a finding to resolve.

   Then invoke the `write-release-notes` skill on that ledger and write `notes.md` with Paneflow's conventions, modeled on the previous release body (`gh release view <previous> --json name,body`):
   - An HTML comment header: `version`, `date`, `previous_version`, `channel`, `breaking`.
   - Title `vX.Y.Z - <subject of five words or fewer>`, kept in `state.md`.
   - An opening `##` section named for the dominant change, then upgrade notes for anything breaking (old behavior, new behavior, who is affected, what to do), then `Added`, `Changed`, `Fixed`, `Removed`, `Security` as the material calls for.
   - A closing `### Install and validation` with placeholders for the run link, the passed legs, the asset count, and a download-size table when sizes moved materially. Fill them in step 6.
   - The `**Full Changelog**` compare link. US English, no em dash glyph, no AI attribution.

   *Criterion:* every slice is marked accepted after your review, the union of their SHAs equals the range, and every user-facing entry maps to a line in `notes.md`.

3. **Bump.** Runbook Step 1: `Cargo.toml` and `Cargo.lock`, the `debian/changelog` stanza, `CHANGELOG.md` (`[Unreleased]` becomes `[X.Y.Z] - YYYY-MM-DD` above a fresh empty `[Unreleased]`, reconciled with the ledger), and the AppStream `<release>` entry (validate with `appstreamcli` when installed). Run the three `AGENTS.md` gates, `cargo test` in a background Bash call, and read the test summary rather than the exit code. Commit `chore: bump version to vX.Y.Z` and push `main`. *Criterion:* the gates are green on the bump commit and `origin/main` equals it.

4. **Tag.** Run `cargo fmt --check` once more on the exact commit, then runbook Step 2. *Criterion:* `git ls-remote --tags origin 'vX.Y.Z^{}'` resolves to the bump commit.

5. **Follow the chain to green.** Find the tag-push `release` run whose `headSha` is the bump commit, and follow it with `gh run watch --exit-status <id>` in a background Bash call; the harness wakes you when it exits. The run takes 25 minutes or more. On each wake, read the real state with `gh run view <id> --json status,conclusion,jobs`; if the watcher ended without a conclusion, arm it again. Any red job goes through [recovery](references/red.md), and the loop continues from there. While the release builds, prepare the site change from [site sync](references/site.md) without pushing it. For a stable tag, then follow `repo_publish` and `update_cask` the same way, matched to this tag, and confirm their job actually ran. *Criterion:* every run in the channel's contract is `success` with its publishing job executed, and the annotations show no signing, lintian, or GPG warning.

6. **Assets and notes.** Runbook Step 4: draft and prerelease flags, `releases/latest` for a stable tag, and the sorted asset list against the expected set. Fill the placeholders in `notes.md` with the verified run URL, legs, and asset count, then publish them with `gh release edit vX.Y.Z --title "<title>" --notes-file tasks/release/vX.Y.Z/notes.md`. *Criterion:* `gh release view vX.Y.Z --json name,body` returns the final title and notes with no placeholder left, and the asset count matches the expected set.

7. **Streams.** Stable only. Runbook Steps 5 and 6: apt `InRelease` date and `Packages` version, rpm `repomd.xml` revision, cask version, then both container installs from `pkg.paneflow.dev`. *Criterion:* both containers print `paneflow X.Y.Z`.

8. **Site.** Stable only. Finish and publish the change prepared in step 5, per [site sync](references/site.md). *Criterion:* the site sync criterion holds.

9. **Report.** Remove the release worktree if you made one, then report under three headings. **Blocked on me**: open decisions or `None`. **Shipped**: version, tag and commit, run URLs with conclusions, release URL and asset count, stream and install evidence, site commit and deployment URL. **Found**: recoveries applied, discrepancies between docs and workflows, warnings accepted and why. Call the release done only when every row of the channel's contract holds.

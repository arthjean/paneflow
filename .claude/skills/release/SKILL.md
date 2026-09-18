---
name: release
description: Publish a Paneflow release and verify its distribution channels.
disable-model-invocation: true
argument-hint: "[version, e.g. 0.8.3]"
---

# Release Paneflow

Target: $ARGUMENTS

Ship the requested release through verified publication. A release request authorizes the release commits, tag push, notes, distribution updates, and stable-site deployment. Carry routine fixes and retries through to completion within that scope. A request to edit or review this skill does not authorize a release. Explicit user scope takes precedence.

## Completion contract

| Channel | Required evidence |
|---|---|
| Stable | Tag-triggered `release` succeeded; GitHub Release is published and latest with the full signed asset set and final notes; `repo_publish` and `update_cask` jobs executed successfully for this tag; apt/rpm installs report the new version and the cask points to it; paneflow.dev serves it from the verified site deployment. |
| `-rc.N`, `-alpha.N`, `-beta.N` | Tag-triggered `release` succeeded; GitHub Release is published as a prerelease with the full signed asset set and final notes. Stable package streams, cask, and site remain on the stable release. |

Incomplete or failed evidence is not success. Continue recoverable work; when credentials, an external outage, or a decision outside the authorized scope prevents progress, report the exact blocker, what is already public, and the next action. Preserve enough release state to resume without repeating publication.

## References by need

- For bumping, tagging, asset verification, and package installation commands, use [the release runbook](../../../docs/release/runbook.md). Repository `AGENTS.md` owns validation requirements; keep its pinned toolchain and exact gate flags when using runbook commands.
- For failures, missing downstream runs, or incorrect published state, read [recovery](references/red.md).
- For stable-site changes and deployment, read [site synchronization](references/site.md). Skip this branch for prereleases.

Paths in the runbook are repository-relative. Resolve shell placeholders explicitly for each tool invocation. When documentation and live workflows disagree, inspect the relevant workflow and explain the discrepancy before any affected publication; preserve the completion contract and repository gates.

## Release identity and scope

Inspect the branch, worktree, remote, and existing tags before switching or pulling. Release merged work from clean, current `main`; preserve unrelated local changes in their checkout and use an isolated checkout when needed. Do not stash or discard Arthur's work automatically.

Normalize an optional leading `v`. Accept `MAJOR.MINOR.PATCH` with an optional `-rc.N`, `-alpha.N`, or `-beta.N` suffix. Refresh remote tags and identify the previous published release relevant to the channel and reachable from the release history. For a stable release, use the previous stable as the notes baseline so prerelease changes remain covered.

If the version is omitted, inspect the change range first: patch for fixes only, minor for user-visible features. State the inferred version; surface breaking changes before choosing their release policy. Check semantic version precedence and remote tag availability. A failed remote lookup is not proof that a tag is available.

Record the version, tag, previous tag, channel, release commit SHA, notes path, and workflow IDs as they become known. A pushed tag is immutable. If resuming an existing release, verify its peeled commit and resume missing work; if the requested version collides with another release or does not advance the channel, resolve that conflict before publishing.

## Changes and notes

Account for every commit in the range as a user-facing change or internal-only. Ground the ledger in the diff, `CHANGELOG.md`, and relevant linked issues. Local task trackers, if available, are supplementary evidence, not a required or versioned source of release history.

Use `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, and `Security` where useful. Include performance, install size, packaging, or build requirements when they affect users; support claims with source or measured evidence.

Use the available `write-release-notes` skill for drafting. Keep these Paneflow conventions:

- Title: `vX.Y.Z - <subject in five words or fewer>`.
- Open with a section named for the dominant change. Explain affected users and actionable upgrade steps for breaking changes.
- Close with `### Install and validation`: downloads, the actual release run link, passed platforms, and asset count. Fill those facts after verification.
- Retain the `**Full Changelog**` compare link. State compatibility for configuration, sessions, terminal backend selection, and packaging only after checking the range.
- Write in US English, without em dash glyphs or AI attribution.

Keep the notes outside the repository. Use the same ledger for the terser changelog and AppStream entry.

## Publication and verification

Prepare the workspace version and lockfile, Debian changelog, `CHANGELOG.md` release entry plus fresh `Unreleased` heading, and AppStream metadata using the runbook. Check that the previous changelog entry exists. Inspect the release-only diff and run the repository's pre-commit gates before committing and pushing the bump.

Run `cargo fmt --check` on the exact commit before tagging, and at the commit/push boundaries required by `AGENTS.md`. Confirm the annotated remote tag's peeled commit equals that release commit.

Follow the tag-triggered release and, for stable releases, its downstream workflows. Match runs to the tag and commit (and downstream resolved tag), not simply the newest run returned by `gh run list`. A successful run with its publishing job skipped does not satisfy a stable release. Inspect signing and asset-verification warnings before accepting success.

Verify the expected asset names, checksum and signature sidecars, and AppImage zsync files against the current workflow and runbook. Confirm published/draft and prerelease state; for stable releases verify the latest-release tag through the GitHub API. Publish the final notes with the verified run and asset evidence.

For stable releases, verify the public apt/rpm metadata and cask, then run the runbook's container installs from the public repositories. Both installs must report the target version. Finish the site synchronization after those distribution checks pass.

Report the version, tag and commit, applicable workflow URLs and conclusions, release URL and asset count, package verification, and stable-site commit/deployment URL. Explain expected prerelease skips and any recovery or unresolved blocker. Claim completion only when the channel's contract is satisfied.

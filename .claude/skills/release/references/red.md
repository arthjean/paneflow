# Release recovery

Read the failed job's log (`gh run view <id> --log-failed`) before choosing a move, and record in `state.md` what is already public: GitHub Release state, apt/rpm, cask, site. Every retry targets this tag; confirm it by tag and head SHA, never by "newest run".

## Where the failure sits decides the move

**Before publication.** The `Publish GitHub Release` job has not published: the release is absent or still a draft, and neither `repo_publish` nor `update_cask` has run for the tag. Nothing has reached a user, so fix and re-tag the same version, as `AGENTS.md` prescribes for a dirty tag:

```bash
git tag -d "v$VERSION"
git push origin ":refs/tags/v$VERSION"
[ "$(gh release view "v$VERSION" --json isDraft --jq .isDraft 2>/dev/null)" = true ] \
  && gh release delete "v$VERSION" --yes
```

The guard deletes a draft and nothing else. Commit the fix on `main`, rerun the gates, push, then tag again from runbook Step 2 at the fix commit. This is the only tag deletion the mandate covers.

**After publication.** The release is public. A source fix ships under the next version, and choosing it is blocker 2 in the skill: commit and gate the fix on `main`, then stop with the proposed version.

## Build, test, signing, or asset failure

| Evidence | Move |
|---|---|
| fmt, clippy, or test failure in `Release test gate` or a build leg | Reproduce locally with the exact `AGENTS.md` gate, fix the source, re-tag (before publication). A `#[cfg(windows)]` clippy failure is invisible on Linux: read the file's item order, the `items_after_test_module` trap in `AGENTS.md` is the usual cause. |
| Failure with no source cause: network fetch, runner crash, a known flaky test | `gh run rerun <id> --failed` once. A second identical failure needs diagnosis. Ignoring or deleting a test is never the fix. |
| Other legs show `cancelled` | They were cancelled by the failed leg. Fix that one; check the cancelled legs on the next run. |
| Missing or invalid signing secret | Read the matching `docs/release/*-signing.md` and `keys/README.md`. Restoring a secret is blocker 1 unless you can do it through `gh secret set` with a value you already hold. Never print a secret value. |
| Runner stays queued | Check GitHub status and runner availability. A queue alone is no reason to cancel. |
| Builds green, release stuck as draft | The asset verification step refused it. Compare expected names, sidecars, and uploads in its log; the fix is a source change, so re-tag. |
| Smoke test or `Auto-update e2e` red after the release was published | Smoke tests run beside the publish job, so the release may be public while the run is red, which keeps `repo_publish` and `update_cask` from firing. An infrastructure cause: `gh run rerun <id> --failed`, and a green rerun fires the downstream chain. A real packaging defect: after publication. The e2e job only informs, but read why it went red before accepting it. |
| Signing, lintian, or GPG `::warning::` | Read it and the verification output. `ENOENT` from the rust-cache post step is noise; a signature or artifact warning is an open row. |

## Distribution workflows

| Evidence | Move |
|---|---|
| Prerelease tag | Expected skip. |
| Parent `release` run red | Repair the parent first. |
| Parent green, downstream missing or its job skipped | Read the parent event and the downstream `if:` guard, then `gh workflow run repo_publish.yml -f tag=vX.Y.Z` or `gh workflow run update_cask.yml -f tag=vX.Y.Z`, and confirm the job ran with that resolved tag. |
| `update_cask` 401 | Known transient: dispatch once with `-f tag=`. A second 401 is a deploy key problem, blocker 1. |
| R2 sync 403 | A credential problem, blocker 1. An empty staging directory must never replace the public repository. |

## Published streams or site

| Evidence | Move |
|---|---|
| apt `InRelease` stale | Cloudflare edge TTL: wait a few minutes and query again. Still stale after about 10 minutes: read the publish and purge logs. |
| `Hash Sum mismatch` or dnf metadata failure | Compare published metadata with package hashes and the publish logs, then dispatch `repo_publish` again. Keep signature and freshness checks on. |
| Container installs an older version | Caches or a concurrent publish. Check the resolved package version, then retry in a fresh container. |
| A prerelease became latest | Restore the prerelease flag and mark the stable latest (`gh release edit`), then check `releases/latest`. |
| Site post-deploy check fails on production | Promote the previous known-good Vercel deployment first, confirm the site recovered, then fix forward on `main`. |

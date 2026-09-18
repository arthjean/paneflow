# Release recovery

Use the failed job logs and the resolved release tag to choose a recovery. Retry an infrastructure failure on the same commit when appropriate. Source changes require a new version and tag after the repository gates pass: preserve pushed tags and existing release history. If choosing a replacement version needs Arthur's decision, prepare the fix and state that decision explicitly.

Resume from verified state. Record which artifacts and channels are already public before retrying a publishing job, and confirm the retry targets this release rather than an older or concurrent release.

## Build, test, signing, or asset failure

| Evidence | Recovery |
|---|---|
| Formatting, clippy, or test failure | Diagnose the failing target, fix the source, and run the applicable repository gates. Publish the correction under a new version; do not delete and recreate the pushed tag. |
| A test passed locally but failed in CI | Compare target, environment, and logs. Retry when evidence supports a transient failure. A repeat failure needs diagnosis; disabling or ignoring a test is not a release workaround. |
| Another matrix leg was cancelled | Start with the original failed leg. Check cancelled required legs on the subsequent successful run. |
| Missing or invalid signing secret | Consult the matching `docs/release/linux-signing.md`, `macos-signing.md`, or `windows-signing.md` and `keys/README.md`. Restore the required credential through the authorized secret-management path, then retry. If access is unavailable, name the credential blocker without exposing secret values. |
| Runner remains queued | Check runner availability and run status. A queue delay alone does not justify cancelling and restarting the release. |
| Build is green but the release remains a draft | Inspect uploaded-asset verification. Compare expected names, sidecars, and uploaded files; a draft blocked by verification must pass that gate before publication. Source fixes require a new tag. |
| Signing, lintian, or GPG warning | Read the warning and relevant verification evidence before accepting the run. Distinguish harmless runner cleanup warnings from an artifact or signature problem. |

## Missing or skipped distribution workflows

Inspect `.github/workflows/repo_publish.yml` or `update_cask.yml` and the triggering release run.

| Evidence | Recovery |
|---|---|
| Prerelease suffix | Expected skip. Keep prereleases off stable apt/rpm, Homebrew, and the stable site. |
| Parent release failed | Repair the release first. Dispatch downstream publication only after that tag's release and assets are verified. |
| Successful parent but downstream missing/skipped | Inspect the actual parent event, resolved tag, and guard. If needed, dispatch the applicable workflow with `-f tag=<verified-stable-tag>`, then verify its resolved tag and executed job. A rerun alone is not proof the event changed. |
| Parent workflow renamed | `workflow_run` matches the declared workflow name. Restore or align the trigger and confirm the repaired chain. |
| R2 sync returns 403 | Check the credential failure against the workflow's required secrets. Restore access through the authorized path. Inspect staging and the sync dry-run before retrying; an empty staging directory must not replace the public repository. |
| Homebrew tap returns 401 | Inspect authentication and retry only if transient. Persistent failure needs credential repair, not repeated dispatches. |

## Published streams or site are wrong

| Evidence | Recovery |
|---|---|
| apt metadata is stale or reports `Hash Sum mismatch` | Allow a short cache propagation interval, then compare published metadata and package hashes. Inspect the publish/purge logs if it persists. Repair or republish consistent metadata while retaining signature and freshness checks. |
| dnf cannot download metadata | Check the configured base URL, public GPG endpoint, and publish logs. Preserve package and repository signature verification. |
| Installed version is wrong | Check caches, resolved package versions, and competing publication runs before reinstalling in a clean container. Verify the retry will not overwrite a newer release. |
| Prerelease became latest | Restore the prerelease flag and the intended stable latest release explicitly, then verify the latest-release API. Inspect the workflow's channel logic before the next release. |
| Critical bug after publication | Report the affected public channels and prepare a corrective patch. Changing GitHub latest does not roll back apt, rpm, Homebrew, or installed clients. Coordinate any withdrawal or rollback outside the release request with Arthur. |
| Site deployment fails production checks | Restore the previous known-good deployment when available, verify recovery, then fix forward. Follow the site's deployment instructions. |

If recovery needs unavailable access or an out-of-scope decision, report the failure with the release/run links, public state, prepared fix, and exact action needed to resume. Do not label the release complete.

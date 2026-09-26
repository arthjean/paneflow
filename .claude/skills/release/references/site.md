# Sync paneflow.dev

Stable releases only. The site is `../paneflow-web` (remote `arthjean/paneflow-web`); read its `AGENTS.md` before editing, it rules there. The checkout often holds Arthur's uncommitted work: stage by explicit pathspec, never `git add -A`.

## Prepare (during step 5)

Model the change on the previous release-sync commit:

```bash
git -C ../paneflow-web log --oneline -3 --grep='chore(release): publish Paneflow'
git -C ../paneflow-web show --stat <previous-sync-commit>
```

Surfaces that move on every release:

- `src/lib/release.ts`: `LATEST_VERSION`, the single source for every download URL.
- `messages/*.json`, all six locales: the hero `availability` line, translated, edited in place (key order is contractual for `check:i18n`; FR copy says "Espaces de travail", never "Workspace").
- `content/docs/installation/windows*.mdx`, six files: the version appears several times in each, and `check:semantic` fails until every one matches `LATEST_VERSION`.

Surfaces that move when the release changed them:

- `content/docs/configuration/schema*.mdx`: a row per new or changed key of `../paneflow/schemas/paneflow.schema.json` (`check:docs` enforces coverage). Port the text from `../paneflow/docs/user/configuration/schema.md`.
- `content/docs/features*.mdx` and other docs pages whose described behavior changed. Trace each claim to the release ledger or `../paneflow/README.md`. Edit the EN source and its locale twins in the same commit, bump `dateModified`, keep `lastSyncedFrom`.
- `src/lib/commands-reference.ts` for new keybinding actions.

Paneflow is GPL-3.0-or-later in all copy; no em dash glyph.

## Publish (step 8, after the streams are verified)

Add the `src/lib/releases.ts` `FALLBACK_PUBLISHED_AT` row keyed `"vX.Y.Z"`, holding `gh release view vX.Y.Z --json publishedAt --jq .publishedAt`. Then:

```bash
bun run check
bun run build
git commit -m "chore(release): publish Paneflow vX.Y.Z"
git push origin main
```

Vercel deploys `main`. Wait for the deployment of that exact commit to be ready: `gh api repos/arthjean/paneflow-web/commits/<sha>/status --jq '.statuses[] | "\(.context): \(.state)"'` reports the Vercel context. Then, from `tasks/deploy-validation-runbook.md` in the site repo:

```bash
bash scripts/post-deploy-i18n-check.sh https://paneflow.dev
curl -fsSL https://paneflow.dev/download | grep -o "paneflow-X.Y.Z[^\"<]*" | sort -u
```

Run `bun run indexnow` only when public URLs were added or removed.

**Criterion:** `bun run check` and `bun run build` passed, the commit is on `origin/main`, its Vercel deployment is ready, the post-deploy script passes, and `/download` links `X.Y.Z` artifacts.

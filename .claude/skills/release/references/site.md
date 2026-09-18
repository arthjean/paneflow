# Sync paneflow.dev

For a stable release, locate the separate `paneflow-web` repository, normally a sibling of this checkout. Confirm its remote before editing. Its `AGENTS.md` is authoritative there; read it before editing. Model the change on the previous release-sync commit:

```bash
git -C <site-checkout> log --oneline -3 -- src/lib/release.ts
```

Confirm these release surfaces against the current checkout and previous release-sync commit:

- `src/lib/release.ts`: `LATEST_VERSION`. Single source for every download URL.
- `src/lib/releases.ts`: a new `FALLBACK_PUBLISHED_AT` row keyed `"$TAG"`, holding the real `publishedAt` from `gh release view "$TAG" --json publishedAt`.
- `messages/*.json`, all six locales: the hero `availability` line and every `/compare` `footnote` that names a Paneflow version. Key order is contractual for `check:i18n`, so edit values in place.

What moves only when behavior changed: `content/docs/**` including its per-locale twins, and `src/lib/compare-meta.ts` `lastModified` when a compare claim actually changed. Trace each claim to `../paneflow/README.md` or `../paneflow/ABOUT.md`.

Keep Paneflow licensed GPL-3.0-or-later in public copy. Follow the site instructions for typography and formatting.

Prepare the site change while the release builds, then publish it after the stable artifacts and streams are verified. Inspect and stage only the release-related diff. Use the current site validation scripts; the existing release pass is:

```bash
bun run check
bun run build
git commit -m "chore(release): publish Paneflow $TAG"
git push
```

Vercel deploys `main` automatically. Verify the deployment belongs to the pushed site commit. After it lands, use the site's deployment instructions; a local `tasks/deploy-validation-runbook.md`, if present, is supplementary. The existing post-deploy pass is: `bash scripts/post-deploy-i18n-check.sh https://paneflow.dev`, then `bun run indexnow` when public URLs changed. If the post-deploy script fails on production, promote the previous known-good Vercel deployment before fixing forward.

**Complete when:** `bun run check` and `bun run build` pass, the commit is pushed, the Vercel deployment is ready, and `https://paneflow.dev` serves the new version.


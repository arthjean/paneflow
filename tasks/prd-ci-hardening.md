[PRD]
# PRD: Durcissement de la CI et de la distribution

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-02 | Arthur Jean | PRD initial, tiré de l'audit des 13 workflows du 2026-11-02 : échecs silencieux rendus bruyants, gates qui disent la vérité, tests instables visibles et corrigés (Phase 1), puis cache, doublons, supply chain, migration Ubuntu 22.04 et décomposition (Phase 2). 6 epics, 26 stories. |
| 1.1 | 2026-11-02 | Arthur Jean | Aucune release Paneflow réelle pendant ce PRD : chaque critère se prouve par dry-run ou par un run qui ne publie rien. US-002 se prouve sur un dry-run de `release.yml`, et US-004 supprime les copies de secrets du dépôt après un dry-run signé sur `main`, sans attendre de tag. |
| 1.2 | 2026-11-03 | Arthur Jean | US-010 : le profil `ci` passe à `retries = 1` avec `flaky-result = "fail"`. Avec `retries = 0`, nextest ne relance jamais un test, qui ne peut donc jamais être FLAKY, ce qui contredisait l'AC « échoue puis passe », FR-05 et l'edge case 6. Seul le test en échec est relancé une fois ; s'il passe, le job échoue quand même. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `d5c17efc1`. L'audit qui fonde ce PRD est une preuve locale, non suivie par git : `tasks/ci-audit/AUDIT.md` et ses rapports détaillés (`release.md`, `run-tests.md`, `libghostty.md`, `distribution.md`, `flaky-forensics.md`, `research-gha-best-practices.md`). Un autre agent ne peut pas les lire depuis le dépôt. Les identifiants entre parenthèses (R-01, T-03, L-01, D-04) renvoient à ces rapports. La fenêtre d'historique va du 2026-09-28 au 2026-11-02.

1. **Des automatismes échouent en silence.** L'audit de sécurité quotidien n'a jamais ouvert d'issue : `audit.yml:147-150` passe les labels `security,automated`, qui n'existent pas, et les 34 échecs sont morts sur `gh issue create` (D-01). Dans les boucles de signature Windows (`release.yml:454-461`, `:1294-1299`), seul le dernier appel de `sign-windows.ps1` compte (R-03). `bash -euxo pipefail` (`repo_publish.yml:54-56`) trace la clé GPG privée complète dans le log public, protégée uniquement par le masquage ligne par ligne (53 lignes `***` dans le run 41428495013, D-04).
2. **Les gates ne disent pas la vérité.** v0.19.0 a été publiée sur `d5c17efc1` alors que `run_tests` 41365552781 était rouge sur ce SHA (R-01) : la release ne teste que Linux x86_64. Le filtre `rust` (`run_tests.yml:80-97`) ne voit que les `*.rs`, donc un changement sous `native/libghostty/**`, `src-app/assets/**` ou `skills/**` ne lance ni clippy ni tests Windows (T-01, L-03). `GHOSTTY_SHA` est recopié dans trois workflows (`libghostty-linux.yml:30`, `libghostty-macos.yml:21`, `release.yml:63`), si bien qu'aucune des 4 PR du bot de bump (#59, #85, #106, #110) n'a pu passer ses propres gates (L-01).
3. **Les tests instables coûtent la moitié des échecs et une partie est invisible.** 44 des 96 runs `run_tests` en échec (45,8 %) ne portent que des tests instables. Le job macOS échoue sur des flaky dans 35 % de ses exécutions. 51 des 129 jobs Windows verts (39,5 %) ne sont passés qu'au 2e ou 3e essai de la boucle `run_tests.yml:993-1021`, sans aucune annotation, pour environ 456 runner-minutes.
4. **Le cache ne sert plus.** Le cache Actions pèse 11,24 GB pour un quota de 10 GB. Les 5,68 GB des caches du tag `v0.19.0` ne sont restaurables par aucun autre ref, et il ne reste aucune entrée `run_tests` sur `main` : tout compile à froid (T-02, R-06).
5. **Les secrets de signature sont trop exposés.** Les environnements `release`, `Production` et `Preview` n'ont aucune règle de protection, et les secrets Apple, Azure, GPG et R2 sont au niveau du dépôt. Des dry-runs dispatchés depuis des branches non mergées ont donc signé avec les identités de production (R-04, D-09).

**Why now:** Ubuntu 22.04 est en dépréciation depuis le 2026-09-17, avec des brownouts du 2027-03-23 au 2027-04-13 et un retrait le 2027-04-17 ([runner-images #14254](https://github.com/actions/runner-images/issues/14254)). 42 lignes `runs-on:` et 2 legs de matrice sont concernés. Node 20 est retiré depuis le 2026-09-23 et cinq actions tournent en Node 24 forcé ([changelog](https://github.blog/changelog/2026-09-23-node-20-is-no-longer-available-in-github-actions/)). Le taux du flaky Windows principal est passé de 12 % à environ 40 % en octobre. Enfin, GitHub a livré en 2026 les primitives qui rendent la correction peu coûteuse : `cache-mode` (GA le 2026-09-10) et `queue: max` (2026-05-07).

## Overview

Le PRD procède en deux phases. La Phase 1 (EP-001 à EP-003) corrige ce qui ment ou se tait. La Phase 2 (EP-004 à EP-006) rend la CI plus rapide, plus sûre et plus simple.

En Phase 1, chaque échec devient visible. L'audit de sécurité ouvre une issue `type: security` et couvre aussi le dernier tag publié. Une signature Windows ratée fait échouer la release, et l'Authenticode de chaque exécutable est vérifié. Plus aucun shell de workflow ne trace avec `-x`, et `scripts/check-workflows.sh` le vérifie mécaniquement. Les secrets de signature passent dans l'environnement `release`, réservé à `main` et aux tags `v*`. Les gates s'alignent sur ce que le build lit réellement :
- un filtre `rust` par répertoire, avec un script qui vérifie que tout `include_str!`, `include!` ou `#[folder]` y est couvert ;
- un seul lecteur du manifest libghostty ;
- une release qui exécute un `preflight` (SHA résolu une fois, invariants de version) et un `ci-verdict`, qui exige le succès des jobs requis de `run_tests` sur le SHA exact au lieu de relancer un gate Linux sériel.

Les tests passent sur cargo-nextest, après un spike de validation. Seuls les tests en quarantaine sont retentés, tout autre flaky fait échouer le run, et le JUnit est uploadé à chaque échec. Les deux flaky actifs sont corrigés à leur cause : la course entre follower et sortie côté Windows, un budget de 600 ms côté macOS. Une règle d'AGENTS.md et un script à cliquet sortent les budgets wall-clock des tests unitaires.

En Phase 2, le cache n'est plus sauvegardé que depuis `main`, et `release.yml` passe en `cache-mode: none`. Chaque suite ne tourne qu'une fois. Le chemin critique de la release descend sous 30 min, contre 48,9 min aujourd'hui. Chaque job déclare son timeout, et les retries réseau couvrent les erreurs observées. Côté supply chain : actions en Node 24, pin checker étendu à tous les workflows, Dependabot, zizmor en CI, vérification minisign avant la publication apt, rpm et cask, attestations de provenance et releases immuables. Le plancher glibc 2.35 est conservé dans un conteneur `ubuntu:22.04` sur des hôtes 24.04, après un spike. `release.yml` (3 464 lignes) devient un orchestrateur de workflows réutilisables, et libghostty n'a plus qu'un workflow de recette.

Décisions clés, avec leur preuve dans l'audit :
- **Consommer le verdict CI** plutôt que relancer les tests comme Zed et WezTerm : c'est moins cher et cela couvre les quatre plateformes.
- **Le flaky n° 1 est un bug de test.** `ARCHITECTURE.md:935-951` promet `-32024` à un follower tardif et le texte final via `session.text`.
- **nextest avec quarantaine**, sans retries globales : `--retries` en CLI désactive les overrides par test ([nexte.st/retries](https://nexte.st/docs/features/retries/)).
- **Plancher glibc gardé.** `packaging/debian/postinst:29` annonce « Ubuntu 22.04+ ».

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Plus aucun échec silencieux | 100 % des advisories RUSTSEC ouvrent ou commentent une issue en moins de 24 h ; 0 ligne de secret multi-ligne tracée dans un log | Inchangé |
| Une release ne publie que du vert | 0 release publiée sur un SHA dont un job requis de `run_tests` n'est pas `success` | Inchangé |
| Tests instables visibles et rares | 0 retry caché ; part des runs en échec ne portant que des flaky inférieure à 15 % | Inférieure à 5 %, quarantaine de 2 tests au plus |
| CI et release plus rapides | Le cache est restauré par au moins 80 % des jobs `run_tests` sur `main` | Release en 30 min au plus ; cache total sous 8 GB |
| Prêt pour le retrait d'Ubuntu 22.04 | Spike conteneur conclu | 0 `runs-on` sur une image 22.04 avant le 2027-03-23 |

## Target Users

### Mainteneur et releaser unique
- **Role:** Arthur, seul contributeur visible, qui coupe les releases par tag `vX.Y.Z` et pousse souvent directement sur `main` (87 % des commits depuis le 2026-10-01).
- **Behaviors:** pousse, regarde `tests_pass`, relance les jobs rouges, coupe un tag quand `main` lui semble sain.
- **Pain points:**
  - un rouge ne veut rien dire la moitié du temps (45,8 % de flaky) ;
  - un vert peut cacher un échec (39,5 % des jobs Windows) ;
  - une release peut partir sur du rouge sans qu'il le voie ;
  - l'audit de sécurité ne l'a jamais prévenu.
- **Current workaround:** relances manuelles (29 runs relancés sur la fenêtre), lecture des logs bruts, `gh run list --commit` à la main avant un tag (`docs/release/runbook.md:97-101`).
- **Success looks like:** un rouge signifie un vrai problème, un vert signifie que tout a passé, et la release refuse d'elle-même un SHA non validé.

### Agents de code qui travaillent dans le dépôt
- **Role:** les sessions Claude Code et Codex qui implémentent des epics et poussent des branches. La CI est leur boucle de feedback.
- **Behaviors:** poussent, attendent la CI, corrigent selon les logs, souvent en parallèle dans le même checkout.
- **Pain points:**
  - un échec instable les envoie corriger un bug qui n'existe pas ;
  - le fail-fast de `cargo test` cache les binaires suivants ;
  - aucune sortie machine (JUnit) n'est disponible.
- **Current workaround:** relire les logs de 1 727 jobs, comme l'a fait l'audit, ou relancer.
- **Success looks like:** chaque test en échec est nommé dans une annotation, avec son statut FLAKY ou FAIL et un artifact JUnit.

### Utilisateurs qui installent Paneflow
- **Role:** les utilisateurs Linux (apt, dnf, zypper, AppImage), macOS (cask) et Windows (MSI).
- **Behaviors:** installent et mettent à jour depuis les dépôts et la release GitHub.
- **Pain points:** ils doivent faire confiance à des artefacts dont la signature des helpers Windows n'est pas vérifiée et dont les métadonnées apt sont signées sans vérification minisign préalable (D-05).
- **Current workaround:** aucun.
- **Success looks like:** chaque artefact publié est signé, vérifié avant publication et attesté (`gh attestation verify`).

## Research Findings

### Competitive Context
- **Zed** (`zed-industries/zed`) : `release.yml` relance tests et clippy sur trois OS à chaque tag, avec `cargo nextest run --workspace --no-fail-fast` et un slow-timeout de 60 s, sans retries ([release.yml](https://raw.githubusercontent.com/zed-industries/zed/main/.github/workflows/release.yml), [nextest.toml](https://raw.githubusercontent.com/zed-industries/zed/main/.config/nextest.toml)). Le cache passe par sccache sur R2, et la signature Azure par `AZURE_CLIENT_SECRET` sans `environment:`. Paneflow consomme le verdict au lieu de tout relancer.
- **Ghostty** : `release-tag.yml` ne teste rien et ne vérifie aucun verdict ; ses secrets de signature sont au niveau du dépôt. Un agrégateur « Required Checks » unique couvre environ 45 jobs ([test.yml](https://raw.githubusercontent.com/ghostty-org/ghostty/main/.github/workflows/test.yml)).
- **WezTerm** : il builde Linux dans des conteneurs de distribution (`ubuntu:22.04`) sur `ubuntu-latest`, ce qui fixe le plancher glibc, et lance `nextest --no-fail-fast` dans ses workflows de tag ([gen_ubuntu22.04_tag.yml](https://raw.githubusercontent.com/wezterm/wezterm/main/.github/workflows/gen_ubuntu22.04_tag.yml)).
- **Alacritty** et **Rio** : pas de gate de verdict, pas de signature protégée par un environnement.
- **Market gap:** aucun projet comparable ne combine un verdict CI consommé par la release, une quarantaine de flaky outillée et des secrets de signature cantonnés à un environnement protégé.

### Best Practices Applied
- Retries nextest par test, `flaky-result = "fail"` et overrides pour la quarantaine, jamais `--retries` en CLI ([retries](https://nexte.st/docs/features/retries/)). Un harness `harness = false` doit implémenter `--list --format terse` ([custom harnesses](https://nexte.st/docs/design/custom-test-harnesses)). Les doctests passent par `cargo test --doc`.
- Cache sauvegardé seulement depuis la branche par défaut (`save-if`). Les caches de tag ne sont restaurables par aucun autre ref ([dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)). `cache-mode: none` sur les workflows de release ([changelog 2026-09-10](https://github.blog/changelog/2026-09-10-control-github-actions-cache-access-with-cache-mode/), audit zizmor `cache-poisoning`).
- `queue: max` pour qu'aucun run en attente d'une chaîne de publication ne soit remplacé ([changelog 2026-05-07](https://github.blog/changelog/2026-05-07-github-actions-concurrency-groups-now-allow-larger-queues/)).
- Les secrets d'environnement ne sont servis qu'aux jobs qui référencent l'environnement, après ses règles de déploiement ([environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)).
- Les checks requis ne doivent pas venir d'un workflow filtrable par chemin : on garde des agrégateurs stables avec `needs` ([required checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)).

*Full research sources available in project documentation.* Ces sources sont dans `tasks/ci-audit/research-gha-best-practices.md`, une preuve locale non suivie par git.

## Assumptions & Constraints

### Assumptions (to validate)
- cargo-nextest exécute la même suite que `cargo test` sur les quatre legs, une fois la cible conpty adaptée. Fondement : un seul `[[test]] harness = false` (`crates/paneflow-host/Cargo.toml:21-24`) et aucun doctest. Validée par US-009.
- Le build Linux de release fonctionne dans `container: ubuntu:22.04` sur un hôte `ubuntu-24.04` ou `ubuntu-24.04-arm`. Fondement : WezTerm, et le smoke `.deb` de `release.yml:1936`. Validée par US-021.
- Un commit de release touche toujours `Cargo.toml` (bump de version), donc le filtre `rust` lance tous les jobs requis sur le SHA tagué. Fondement : `**/Cargo.toml` dans le filtre (`run_tests.yml:83`) et le commit `7f3cd8179`.
- Retirer le rebuild libghostty de la release est sûr, parce que l'archive est vérifiée par hash contre le manifest (`scripts/fetch-libghostty.sh:87-95`) et que `libghostty-linux.yml` vérifie la reproductibilité chaque nuit.
- Les flaky macOS viennent surtout des budgets wall-clock sur des runners M1 à 3 vCPU. Fondement : les 15 échecs de `host_status_bounds` se situent entre 603 et 648 ms pour un seuil de 600 ms.

### Hard Constraints
- Les quatre cibles livrées restent couvertes : Linux x86_64, Linux aarch64, macOS aarch64, Windows x86_64 (AGENTS.md).
- Aucun job sur `ubuntu-22.04` ou `ubuntu-22.04-arm` après le 2027-03-23, premier brownout.
- `--locked` sur chaque invocation cargo. Les plafonds de taille des helpers (`paneflow-shim` 512 KiB, `paneflow-ai-hook` environ 375 KB, `paneflow-mcp` 512 KiB) restent appliqués.
- Pas de commentaire dans les scripts shell et PowerShell (AGENTS.md, « No comments in source code »).
- Une release se coupe uniquement sur `/release <version>` d'Arthur : aucun agent ne tague ni ne publie.
- **Aucune release Paneflow réelle pendant ce PRD.** Aucun tag `v*` n'est poussé, et aucun run de `release.yml` (job `Publish GitHub Release`), `repo_publish.yml` ou `update_cask.yml` ne publie, avant que les 6 epics soient `DONE`. D'autres tâches précèdent ensuite la prochaine release. Chaque critère se prouve donc par un `workflow_dispatch` dry-run de `release.yml`, par un run refusé ou sans effet de publication, ou par un test de script sur fixtures. Un critère qui décrit un chemin de tag ou de publication se prouve sur ce chemin rejoué hors publication, et son observation sur une vraie release est un contrôle de suivi après le PRD, pas une condition de `DONE`.
- Les réglages externes du dépôt (environnements, ruleset `main: required CI`, releases immuables, secrets) sont appliqués par Arthur. Les stories fournissent les commandes exactes.
- Le code `#[cfg(windows)]` et PowerShell se vérifie sur un runner Windows réel, et chaque story dit lequel.
- Arthur est le seul contributeur visible : pas d'attribution IA. On écrit `Refs #...`, jamais de mot-clé de fermeture automatique.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - format, porte obligatoire avant tout commit et tout push qui touche du Rust
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint, cibles de test comprises
- `cargo test --workspace --locked` - suite complète ; remplacée par `cargo nextest run --workspace --locked --profile ci` une fois US-010 livrée
- `cargo deny check advisories licenses sources` - seulement quand des dépendances changent
- `bash scripts/check-workflow-action-pins.sh` - pins SHA des actions ; renommé `bash scripts/check-workflows.sh` par US-002, puis étendu par US-017 et US-018

Pour une story qui touche `.github/**` ou un script appelé par un workflow :
- Un run réel des workflows touchés, sur une branche poussée (PR ou `workflow_dispatch`), doit être vert. Le push demande l'autorisation d'Arthur. Pour `release.yml`, ce run est un dry-run ; `repo_publish.yml` et `update_cask.yml` publient à chaque run réussi, ils se vérifient donc par `scripts/check-workflows.sh`, actionlint et un dispatch refusé par l'environnement, jamais par une publication (voir Hard Constraints).
- Le code PowerShell et `#[cfg(windows)]` est vérifié sur un runner Windows réel ou sur l'hôte Windows d'Arthur, et la story dit lequel.

## Epics & User Stories

### EP-001: Les échecs silencieux deviennent bruyants

Phase 1. Chaque automatisme de sécurité ou de signature qui peut échouer le fait de façon visible, et aucun secret n'apparaît dans un log.

**Definition of Done:**
- Une advisory RUSTSEC ouvre ou commente une issue `type: security` en moins de 24 h, pour `main` et pour le dernier tag publié.
- Une signature Windows ratée fait échouer la release avant tout upload.
- Aucun shell de workflow ne trace avec `-x`.
- Les secrets de signature et de publication ne sont servis qu'aux jobs de l'environnement `release`.

#### US-001: L'audit de sécurité quotidien prévient vraiment
**Description:** As a mainteneur, I want qu'une advisory sur `main` ou sur la dernière release publiée ouvre ou commente une issue so that une vulnérabilité livrée aux utilisateurs soit connue sous 24 h (D-01, D-07, D-11, T-09).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given une advisory RUSTSEC détectée par cargo-deny sur `main`, when le cron de `audit.yml` s'exécute, then une issue est créée avec le label existant `type: security`, au format de titre actuel (`audit.yml:140-150`).
- [ ] Given une issue ouverte existe déjà pour la même advisory, when le cron repasse, then il la commente au lieu d'en créer une seconde.
- [ ] Given le dernier tag publié (`gh release view --json tagName`) embarque un crate vulnérable que `main` n'a plus, when le cron s'exécute, then l'issue nomme ce tag. L'audit tourne en matrice sur `main` et ce tag.
- [ ] Given `gh issue create` ou `gh issue comment` échoue (label absent, erreur d'API), when l'étape s'exécute, then le job échoue avec l'erreur de `gh` dans le log, et le résumé d'étape indique que l'alerte n'a pas été déposée.
- [ ] Given la sortie de cargo-deny contient `error[vulnerability]` ou `error[unmaintained]`, when le wrapper de retry s'exécute, then il ne relance pas et n'annonce pas de rate limit probable.
- [ ] Given la clé de signature des dépôts (`packaging/paneflow-release.asc`, expiration 2028-04-19) expire dans moins de 60 jours, when le cron s'exécute, then il ouvre ou commente une issue `type: security`.

#### US-002: Aucun secret n'est tracé par le shell
**Description:** As a mainteneur, I want qu'aucun step de workflow ne s'exécute avec `set -x` so that la clé GPG privée et les autres secrets multi-lignes ne soient jamais écrits dans un log public, même masqués (D-04, R-16).

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `repo_publish.yml:54-56` et tout autre `defaults.run.shell` ou `shell:` de `.github/workflows/` et `.github/actions/` utilisent `bash -euo pipefail {0}`, sans `-x`.
- [ ] `scripts/check-workflow-action-pins.sh` est renommé `scripts/check-workflows.sh`. Il garde ses contrôles de pins et ajoute un contrôle qui échoue sur toute option `-x`, `set -x` ou `set -o xtrace` dans un workflow ou une action composite. Tous ses appelants et la doc sont mis à jour.
- [ ] Given une PR qui ajoute `shell: bash -euxo pipefail {0}`, when `scripts/check-workflows.sh` s'exécute, then il échoue en nommant `fichier:ligne`.
- [ ] Given un dry-run de `release.yml` sur `main` après la livraison, dont le step « Sign .deb and .rpm with release key » importe `GPG_PRIVATE_KEY`, when son log est téléchargé (`gh run view --log`), then il ne contient aucune ligne `***` seule issue de `GPG_PRIVATE_KEY` ; avant, le run `repo_publish` 41428495013 en contenait 53. `repo_publish.yml` n'est pas relancé, puisqu'il publie : son absence de `-x` est prouvée par `scripts/check-workflows.sh`, et la lecture de son log est un contrôle de suivi à la première release après ce PRD.
- [ ] Les contrôles de présence de secret n'utilisent plus `[ -z "$SECRET" ]` sur la valeur elle-même mais `[ -z "${SECRET:+set}" ]`.

#### US-003: Une signature Windows ratée fait échouer la release
**Description:** As a utilisateur Windows, I want que chaque exécutable livré soit vérifié signé avant upload so that un helper non signé ne parte jamais dans un MSI signé (R-03).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `scripts/sign-windows.ps1` sort en non-zéro pour un fichier, when la boucle de signature de `release.yml:454-461` ou `:1294-1299` s'exécute, then le step échoue tout de suite en nommant le fichier et le code de sortie.
- [ ] Given le MSI est construit, when le step de vérification s'exécute, then `Get-AuthenticodeSignature` renvoie `Valid` pour `paneflow.exe`, `paneflow-host.exe`, les trois helpers (`paneflow-shim.exe`, `paneflow-ai-hook.exe`, `paneflow-mcp.exe`), chaque copie du shim mise en place sous `target/x86_64-pc-windows-msvc/release/paneflow-helpers`, et le MSI.
- [ ] Given l'un de ces fichiers n'est pas signé ou porte une signature invalide, then le job échoue avant tout upload d'artifact et liste chaque fichier fautif.
- [ ] Given les secrets Azure sont absents (dry-run sans environnement), then signature et vérification sont sautées avec un avertissement, et le run reste en mode dry-run sans publication, comme aujourd'hui (`release.yml:426-431`).
- [ ] La vérification a tourné sur un runner `windows-2022` réel, via un `workflow_dispatch` dry-run de `release.yml`.

#### US-004: Les secrets de signature vivent dans l'environnement `release`
**Description:** As a mainteneur, I want que les secrets de signature et de publication ne soient servis qu'aux jobs de `main` ou d'un tag `v*` so that une branche non mergée ne puisse plus signer avec les identités de production (R-04, D-09).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `scripts/configure-release-environment.sh` contient les commandes `gh api` exactes. Elles créent la deployment policy de `release` (branche `main`, tags `v*`) et listent les secrets à recréer dans l'environnement puis à supprimer du dépôt : `APPLE_*`, `AZURE_*`, `GPG_*`, `R2_*`, `HOMEBREW_TAP_DEPLOY_KEY`, `CLOUDFLARE_*`. Arthur les applique, et `docs/release/runbook.md` le documente.
- [ ] Les jobs de `release.yml`, `repo_publish.yml` et `update_cask.yml` qui consomment ces secrets déclarent `environment: release`, et uniquement eux.
- [ ] Given un `workflow_dispatch` de `release.yml` depuis une branche autre que `main`, when le build s'exécute, then il produit des artefacts non signés, n'atteint jamais la publication et n'échoue pas sur la deployment policy. Il en va de même d'un dispatch depuis `main` dont `target_ref` n'est pas `main` : la policy juge la ref de dispatch, pas le code construit.
- [ ] Les secrets n'apparaissent que dans l'`env:` des steps qui s'en servent. Aucun `env:` de job ne porte les clés R2 (`repo_publish.yml:80-100`), et `AZURE_CLIENT_SECRET` n'est plus dans l'`env:` d'un step qui lance `cargo build` (`release.yml:413-446`).
- [ ] Given un job qui référence `release` depuis une ref non autorisée, when il démarre, then GitHub refuse le déploiement et aucun secret n'est servi. Un dispatch de `repo_publish.yml` depuis une branche, une fois la policy appliquée, le vérifie sans rien publier.
- [ ] Sans release réelle (Hard Constraints), l'ordre de migration est : policy et secrets recréés dans `release`, puis un dry-run de `release.yml` sur `main` qui signe chaque leg, puis la suppression des copies du dépôt, puis le dispatch de branche du critère 3. Tant que les copies du dépôt existent, un job sans environnement les reçoit encore et signe, donc le critère 3 ne se prouve qu'après la suppression.

---

### EP-002: Les gates disent la vérité

Phase 1. Ce que la CI valide correspond à ce que le build lit et à ce que la release publie.

**Definition of Done:**
- Tout fichier lu par le build déclenche les jobs Rust de `run_tests`.
- Les pins libghostty ne sont lus qu'à un endroit, et une PR de bump passe ses gates.
- `release.yml` ne publie qu'un SHA dont les jobs requis de `run_tests` sont `success`.
- Une annulation n'apparaît plus comme un check requis en échec.

#### US-005: Les filtres de chemins couvrent tout ce que le build lit
**Description:** As a mainteneur, I want que tout changement d'une entrée du build lance clippy et les tests des quatre plateformes so that un re-pin libghostty, une police embarquée ou un skill inclus ne passent plus sans test (T-01, L-03, L-06, D-02).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le filtre `rust` de `run_tests.yml` matche par répertoire : `src-app/**`, `crates/**`, `native/**`, `skills/**`, `bench/baselines/**`, `protocol/**`, `schemas/**`, `examples/**`, `runtimes/**`, `mcps/**`, `fuzz/**`, `**/Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`. L'output `libghostty` disparaît de `run_tests.yml`.
- [ ] Given un commit qui ne modifie que `native/libghostty/manifest.toml`, when `run_tests` s'exécute, then `check_style`, `run_tests_linux`, `macos_check` et `windows_check` tournent.
- [ ] `scripts/check-ci-filter-coverage.sh` extrait tout chemin cité par `include_str!`, `include_bytes!`, `include!`, `#[folder = ...]` et `cargo:rerun-if-changed` dans le workspace, puis échoue si un chemin n'est pas couvert par le filtre `rust`. Le job `check_style` le lance.
- [ ] Given une PR qui ajoute `include_str!("../../docs/notes.md")`, when le script s'exécute, then il échoue en nommant le chemin et le fichier source.
- [ ] Given un commit qui ne touche que `docs/**`, then aucun job Rust ne tourne et `tests_pass` passe sur des jobs `skipped`.
- [ ] `perf-ab.yml` déclenche aussi sur `crates/paneflow-terminal-ghostty/**` et `native/libghostty/**`.
- [ ] Les lanes moteur de `libghostty-linux.yml` (reproductibilité, bindings, fuzz) se déclenchent sur un filtre `recipe` (`native/libghostty/**`, `scripts/*libghostty*`, `crates/paneflow-libghostty-sys/**`, le workflow lui-même), et non plus sur `src-app/src/terminal/**`. Les lanes consommatrices gardent leurs déclencheurs.

#### US-006: Les pins libghostty viennent du seul manifest
**Description:** As a mainteneur, I want que chaque workflow lise le SHA Ghostty, Zig et les toolchains depuis `native/libghostty/manifest.toml` so that une PR de bump qui ne modifie que le manifest passe ses gates (L-01, L-12, L-13, L-14, L-15).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Un lecteur unique (`scripts/libghostty-manifest.sh <clé>`, exposé par `.github/actions/libghostty-manifest` en outputs) fournit `source_sha`, la version de Zig, la toolchain LLVM et la toolchain Rust du build moteur. Il est utilisé par `libghostty-linux.yml`, `libghostty-macos.yml`, `libghostty-windows.yml`, `libghostty-bump.yml` et `release.yml`.
- [ ] Aucun workflow ne déclare plus `GHOSTTY_SHA:` en dur. `libghostty-macos.yml:128,141` ne code plus `1.96.1` en dur. `scripts/check-workflows.sh` échoue sur un `GHOSTTY_SHA:` littéral.
- [ ] Given une clé absente du manifest, when le lecteur s'exécute, then il sort en non-zéro en nommant la clé, et le job échoue.
- [ ] Given une PR de bump ouverte par `libghostty-bump.yml` qui ne modifie que `native/libghostty/**`, when ses checks s'exécutent, then le contrôle de pin passe et `run_tests` lance sa matrice complète (avec US-005).
- [ ] Dans `libghostty-bump.yml`, la vérification d'existence de la branche (`:629-632`) précède la publication de la release et de l'attestation (`:481`, `:573`). Given la branche de bump existe déjà, then le workflow échoue avant de publier quoi que ce soit.
- [ ] Les steps `cargo build` du bump tournent dans un job sans `contents: write` ni `id-token: write`, et le job qui pousse la branche n'exécute aucun build script.

#### US-007: La release ne publie qu'un SHA validé par la CI
**Description:** As a mainteneur, I want que la release résolve son SHA une seule fois et exige les jobs requis de `run_tests` verts sur ce SHA so that une release ne parte plus jamais sur du rouge (R-01, R-02, R-08).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Un job `preflight`, premier de `release.yml`, résout `RELEASE_SHA` une seule fois : `github.sha` sur un push de tag, ou le SHA complet de `target_ref` sur un dispatch. Il rejette un SHA abrégé avec un message explicite (cas du run 36834464356).
- [ ] `preflight` vérifie que le tag vaut `v` suivi de la version de `Cargo.toml`, et qu'en dispatch `version` et `msi_upgrade_version` respectent les règles de mise à niveau du MSI. Une violation échoue en moins de 3 min, au lieu de 38 min dans le run 36829648509.
- [ ] Chaque checkout de `release.yml` utilise `ref: ${{ needs.preflight.outputs.sha }}`, et `RELEASE_REF` (`release.yml:79`) disparaît.
- [ ] Un job `ci-verdict` lit les check runs de `RELEASE_SHA` (`gh api repos/{owner}/{repo}/commits/{sha}/check-runs`) et exige `success` pour chaque job requis de `run_tests` : Style, Tests Linux x86_64, Linux aarch64, macOS aarch64 smoke build, Windows x86_64 libghostty check, Performance gates, security audit. Les noms exacts sont lus dans `run_tests.yml`.
- [ ] Given `run_tests` est encore en cours sur le SHA, when `ci-verdict` s'exécute, then il interroge toutes les 60 s pendant au plus 60 min, et passe dès que tous les jobs requis sont `success`.
- [ ] Given un job requis est `failure`, `cancelled` ou `skipped`, then `ci-verdict` échoue avec le nom du job, son URL et la commande de reprise `gh workflow run run_tests.yml --ref <tag>`.
- [ ] Le job de publication a `needs: ci-verdict`, et les builds tournent en parallèle de `ci-verdict`. Le job `Release test gate (Linux x86_64)` (`release.yml:91-124`) et les steps `cargo fmt --check` et `cargo clippy` de chaque leg de build sont supprimés.
- [ ] Given le tag a été déplacé sur un autre commit pendant le run, when la publication démarre, then elle échoue parce que le tag ne résout plus vers `RELEASE_SHA`.
- [ ] Le paragraphe fmt d'AGENTS.md (« The release pipeline runs the same check on all four Build jobs », lignes 22-24) et les étapes 3 à 5 de `docs/release/runbook.md` décrivent le nouveau gate.

#### US-008: Concurrency et agrégateurs reflètent l'état réel
**Description:** As a mainteneur, I want que le nightly libghostty ne soit plus tué par un push et qu'une annulation ne rougisse plus un check requis so that le nightly tourne la nuit et que le rouge reste un vrai signal (L-02, L-09).

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les runs `schedule` de `libghostty-linux.yml`, `libghostty-macos.yml` et `libghostty-windows.yml` utilisent un groupe de concurrency distinct (`${{ github.workflow }}-schedule`). Given un push sur `main` pendant le nightly, then le nightly continue.
- [ ] Les jobs agrégateurs (`libghostty-linux.yml:585`, `libghostty-macos.yml:260`, `libghostty-windows.yml:131`, `tests_pass` de `run_tests.yml:1381`) utilisent `if: ${{ !cancelled() }}`. Given un run annulé par un push plus récent, then l'agrégateur ne rapporte pas `failure`.
- [ ] Given une dépendance de l'agrégateur en échec, then l'agrégateur s'exécute et échoue.
- [ ] Les groupes de concurrency de `release.yml`, `repo_publish.yml`, `update_cask.yml` et `libghostty-bump.yml` déclarent `queue: max`. Given deux tags poussés à moins d'une minute d'écart, then les deux chaînes s'exécutent dans l'ordre, sans qu'aucune ne soit remplacée.

---

### EP-003: Les tests instables sont visibles, bornés et corrigés

Phase 1. Un job vert ne cache plus aucun échec. Un flaky hors quarantaine fait échouer le run. Les deux flaky actifs sont corrigés à leur cause.

**Definition of Done:**
- Les quatre legs de test tournent sous nextest, avec une quarantaine d'au plus 4 tests, chacun lié à une issue, et un JUnit à chaque échec.
- `an_exit_without_output_ends_the_follower_and_leaves_an_empty_completed_text` et `host_status_bounds_the_memory_reads_of_sixty_four_silent_sessions` ne figurent plus dans la quarantaine.
- Aucun nouveau budget wall-clock n'entre dans un test unitaire.

#### US-009: Validate assumption: cargo-nextest exécute toute la suite sur les quatre legs
**Description:** As a mainteneur, I want prouver que nextest lance les mêmes tests que `cargo test` sur Linux x86_64, Linux aarch64, macOS aarch64 et Windows x86_64 so that la migration de US-010 ne perde aucun test et n'en casse aucun.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] La cible `crates/paneflow-host/tests/conpty.rs` (`harness = false`, qui ignore `--list`) passe en `harness = true` avec un seul `#[test]` qui conserve la ré-exécution `--cursor-child`. À défaut, elle est exclue de nextest et lancée par `cargo test -p paneflow-host --test conpty --locked` sous Windows. Le choix et sa raison sont consignés dans la PR.
- [ ] Sur chacun des quatre legs (run de branche), `cargo nextest run --workspace --locked --no-fail-fast` exécute le même nombre de tests que `cargo test --workspace --locked`. L'écart, compté dans les deux logs, vaut 0 ou est expliqué test par test.
- [ ] Given 5 runs consécutifs par leg, when un test n'échoue que sous nextest, then il est listé avec sa cause (état de processus, variable d'env, répertoire courant), et la validation échoue tant qu'il n'est pas corrigé.
- [ ] `cargo test --workspace --doc --locked` est lancé une fois. Son compte de doctests est consigné : s'il vaut 0, aucun step de doctest n'est ajouté ; sinon, US-010 ajoute ce step.
- [ ] Les temps de mur par leg, nextest et `cargo test`, sont consignés pour comparaison.

#### US-010: nextest remplace la boucle de retry, avec quarantaine et rapport
**Description:** As a agent de code, I want que chaque test en échec ou instable soit nommé dans une annotation et un JUnit so that je corrige le bon test sans relire 1 700 logs (T-03, T-10).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] `.config/nextest.toml` définit un profil `ci` : `retries = 1`, `flaky-result = "fail"`, `fail-fast = false`, `slow-timeout = { period = "60s", terminate-after = 5 }` et un chemin JUnit.
- [ ] La quarantaine est faite d'overrides `[[profile.ci.overrides]]`, un par test, avec `retries = 2`, `flaky-result = "pass"` et le numéro de l'issue de suivi. Elle compte au plus 4 tests au départ : ceux qui ont échoué au moins 3 fois sur la fenêtre de l'audit et que US-011 et US-012 ne corrigent pas.
- [ ] Les quatre legs de test de `run_tests.yml` lancent `cargo nextest run --workspace --locked --profile ci`, nextest étant installé par `taiki-e/install-action` épinglé par SHA. La boucle PowerShell de `run_tests.yml:993-1021` disparaît, et aucun appel ne passe `--retries` ni `NEXTEST_RETRIES`.
- [ ] Given un test hors quarantaine qui échoue puis passe, when le run se termine, then le job échoue et une annotation `::error` nomme le test FLAKY.
- [ ] Given un test en quarantaine qui échoue puis passe, then le job passe, une annotation `::warning` le nomme et une ligne s'ajoute à `$GITHUB_STEP_SUMMARY`. `scripts/ci-test-report.sh` produit ces deux sorties à partir du JUnit.
- [ ] Given un test qui dépasse `slow-timeout` × `terminate-after`, then il est tué et rapporté TIMEOUT, et le job échoue. Sous Windows, le job object de nextest tue aussi ses processus enfants.
- [ ] Given un échec de test, then le JUnit et la sortie du test en échec sont uploadés dans l'artifact `test-results-<leg>`, avec une rétention de 14 jours.
- [ ] Le bloc « Gates you must not skip » d'AGENTS.md cite la commande nextest, et le paragraphe sur les retries Windows est retiré.

#### US-011: Les tests Windows du host n'attendent plus une course
**Description:** As a mainteneur, I want que les tests de suivi de sortie attachent leur follower avant que l'enfant puisse sortir, et cherchent leur marqueur dans le texte visible, so that le flaky Windows le plus coûteux (24 à 40 % des tentatives) disparaisse sans affaiblir le contrat du host.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Dans `an_exit_without_output_ends_the_follower_and_leaves_an_empty_completed_text` (`crates/paneflow-host/src/server.rs:2495`), le follower est attaché avant que l'enfant puisse sortir : le shell attend une ligne sur stdin, puis le test envoie `exit`. Le contrat d'`ARCHITECTURE.md:935-951` (un follower tardif reçoit `-32024`) et l'assertion NFR-04 (runtime libéré en 5 s, `server.rs:2532-2536`) restent inchangés.
- [ ] Given un patch de validation, non commité, qui insère une attente de 1,5 s avant l'abonnement du follower, when le test tourne sous Windows, then il passe, ce qui prouve son indépendance vis-à-vis de `TAIL_RELEASE_GRACE` (`runtime.rs:38`).
- [ ] `read_until_marker` (`server.rs:2318`) cherche le marqueur dans le texte visible, hors des séquences OSC et de titre de fenêtre. `repeated_disconnects...` et `a_connection_lost...` passent.
- [ ] Given la boucle de reproduction de `flaky-forensics.md` §5.1 (200 runs isolés, 30 runs du module `server::tests`) sur l'hôte Windows d'Arthur, then 0 échec des tests modifiés.
- [ ] Given un host qui renverrait `-32024` à un follower attaché avant la sortie, then le test échoue : la violation de contrat reste détectée.
- [ ] Aucune modification de `runtime.rs` ni de `host.rs`. Les changements restent dans `mod tests` de `server.rs`, et tout helper ajouté hors de ce module est déclaré avant lui (AGENTS.md, `items_after_test_module`).

#### US-012: Pas de SLA temporel dans un test unitaire
**Description:** As a mainteneur, I want que les tests unitaires vérifient un résultat sous un délai généreux et que les budgets de latence vivent dans les perf gates so that le leg macOS cesse d'échouer sur des dépassements de 3 à 48 ms.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `host_status_bounds_the_memory_reads_of_sixty_four_silent_sessions` (`crates/paneflow-host/src/host/tests.rs:2940`) vérifie `elapsed < MEMORY_USAGE_BUDGET + 1 s`. Le SLA de 600 ms rejoint les perf gates (`scripts/perf-gates.sh`, `crates/paneflow-host/tests/persistent_baseline/gates.rs`), sur la classe de machine calibrée de `perf_gates`.
- [ ] Chaque assertion wall-clock qui a échoué en CI entre le 2026-09-28 et le 2026-11-02 (liste de `flaky-forensics.md` §3) est convertie en assertion de résultat ou déplacée dans les perf gates.
- [ ] AGENTS.md énonce la règle : un test unitaire vérifie un résultat sous un délai généreux, et un budget de latence vit dans les perf gates.
- [ ] `scripts/check-test-wallclock.sh` échoue quand une nouvelle assertion qui compare `elapsed()` à une `Duration` apparaît dans du code de test hors de `scripts/wallclock-allowlist.txt`. L'allowlist part des 46 occurrences actuelles, moins celles qui sont converties. Le job `check_style` le lance.
- [ ] Given une PR qui ajoute `assert!(started.elapsed() < Duration::from_millis(200))` dans un `#[test]`, when `check_style` s'exécute, then il échoue en nommant `fichier:ligne`.
- [ ] Given 20 runs consécutifs du leg macOS, then 0 échec des tests convertis.

#### US-013: Le smoke visuel ne dépend plus des tests unitaires
**Description:** As a mainteneur, I want que les smokes de rendu macOS et Windows tournent même quand un test unitaire échoue so that un flaky ne désactive plus la garde visuelle (T-04).

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Dans `macos_check` et `windows_check`, le build release et l'upload de l'artifact binaire (`run_tests.yml:614-632` côté macOS) passent avant les tests unitaires.
- [ ] Given des tests unitaires en échec et un artifact uploadé, when `macos_render_smoke` ou `windows_render_smoke` s'évalue, then le smoke tourne et rend son propre verdict ; `tests_pass` échoue quand même à cause des tests.
- [ ] Given un build en échec, then le smoke est sauté et `tests_pass` échoue.
- [ ] Le run 40388529885, où le smoke a été sauté, sert de scénario de non-régression : avec un test unitaire en échec forcé sur une branche, le smoke tourne.

---

### EP-004: Moins de minutes, un cache qui sert

Phase 2. La CI et la release ne paient plus deux fois le même travail, et le cache est restauré au lieu d'être évincé.

**Definition of Done:**
- Au moins 80 % des jobs `run_tests` sur `main` restaurent un cache, et le cache total reste sous 8 GB une semaine après la livraison.
- Une release dry-run sur `main` tient en 30 min au plus.
- Chaque job de chaque workflow déclare `timeout-minutes`.
- Aucune suite de tests n'est exécutée par deux workflows.

#### US-014: Une seule politique de cache
**Description:** As a mainteneur, I want que le cache ne soit sauvegardé que depuis `main` et jamais utilisé par la release so that les builds de `main` et des PR restaurent un cache et que la release se construise d'un état propre (T-02, R-06, L-10, D-08).

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Chaque step `Swatinem/rust-cache` hors release déclare `save-if: ${{ github.ref == 'refs/heads/main' }}`.
- [ ] Given une PR qui ajoute un step `Swatinem/rust-cache` sans `save-if`, when `scripts/check-workflows.sh` s'exécute, then il échoue en nommant `fichier:ligne`.
- [ ] `release.yml` déclare `cache-mode: none` au niveau du workflow, et ses steps `rust-cache` (`:118-121`, `:324-327`) disparaissent. Les findings zizmor `cache-poisoning` de `release.yml:118,225,324` disparaissent aussi.
- [ ] Les caches de `audit.yml:53-62` et `perf-ab.yml:68-78` sont soit restaurés (clés alignées pour que `main` les relise), soit supprimés.
- [ ] Given un run de PR, when `rust-cache` termine, then aucune entrée de cache n'est créée pour la ref de la PR (vérifié par `gh api repos/{owner}/{repo}/actions/caches`).
- [ ] Given deux pushes successifs sur `main` après livraison, then chaque job `run_tests` du second affiche « Cache restored » dans son log.

#### US-015: Une suite ne tourne qu'une fois
**Description:** As a mainteneur, I want supprimer les jobs qui refont ce qu'un autre workflow fait déjà so that environ 7 000 runner-minutes par 8 semaines (estimation de `libghostty.md`) ne soient plus dépensées en double (T-05, T-08, L-07, L-08, L-11).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] `release_build_linux` (`run_tests.yml:233-354`) disparaît. Les plafonds de taille par helper sont vérifiés par un script après les tests de `run_tests_linux`, sur les binaires `release-min` que `src-app/build.rs` construit déjà. La ligne d'AGENTS.md sur le budget de taille nomme le nouveau job.
- [ ] Given un helper qui dépasse son plafond, when `run_tests_linux` s'exécute, then il échoue en nommant le binaire, sa taille et le plafond.
- [ ] `libghostty-linux.yml` ne lance plus ni `cargo test --workspace`, ni release build, ni cargo-deny (`:207-221`, `:561-583`) : ces gates restent dans `run_tests.yml` et `audit.yml`.
- [ ] La lane consommatrice de `libghostty-macos.yml` (`:216-219`) réutilise l'artifact de `macos_check` ou disparaît, et la lane `bindings` ne fait plus de troisième build Zig (`libghostty-linux.yml:367-375`).
- [ ] cargo-deny s'installe en binaire précompilé (version et checksum épinglés), depuis une seule source de version partagée par `run_tests.yml` et `audit.yml`.
- [ ] Given une PR sans effet sur le code, then les checks requis `libghostty validation` restent verts.

#### US-016: Le chemin critique de la release passe sous 30 min
**Description:** As a mainteneur, I want que la release ne rebuilde plus libghostty, ne s'arrête plus en bloc sur un leg et lance le smoke d'upgrade MSI à chaque tag so that une release prenne 30 min au plus et protège la régression 1611 (R-05, R-07, R-09).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Les legs Linux de `release.yml` ne rebuildent plus libghostty depuis la source Ghostty (`:209-227`, `:341-348`). Ils utilisent l'archive vérifiée par hash par `fetch-libghostty`.
- [ ] La matrice de build passe en `fail-fast: false`, et la publication exige toujours tous les legs. Given un leg en échec, then les autres terminent, et « Re-run failed jobs » ne rebuilde que le leg en échec.
- [ ] Chaque smoke ne dépend que du build de sa plateforme.
- [ ] Le smoke d'upgrade MSI avec host vivant (`release.yml:2948-2951`) tourne aussi sur un push de tag et figure dans les `needs` de la publication.
- [ ] Un dry-run de `release.yml` sur `main` se termine en 30 min au plus, contre 48,9 min pour le run 41421859986, mesuré du démarrage du premier job à la fin du dernier.

#### US-017: Timeouts et retries réseau partout
**Description:** As a mainteneur, I want que chaque job ait un timeout et que chaque téléchargement survive aux erreurs transitoires observées so that un step bloqué ne tienne plus un runner 6 h et qu'un hoquet TLS ne fasse plus échouer un job (T-07, R-10, D-14, L-04, L-05, R-14, T-13).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Chaque job de `.github/workflows/` déclare `timeout-minutes`, au plus 1,5 fois la durée maximale observée sur la fenêtre de l'audit, et 60 min par défaut pour un job sans historique. `scripts/check-workflows.sh` échoue sur un job qui n'en déclare pas.
- [ ] `scripts/fetch-libghostty.sh` et `.ps1` appellent curl avec `--retry 5 --retry-all-errors`, plus `--ssl-revoke-best-effort` sous Windows, pour le cas `CRYPT_E_REVOCATION_OFFLINE` du run 36606332042.
- [ ] Le motif `TRANSIENT` de `scripts/ci-retry-network.sh:7` couvre `ReadFailed` et `ProtocolError` de Zig, et les fetchs de paquets Zig des builds moteur passent par ce script.
- [ ] `.github/actions/linux-build-deps` configure `Acquire::Retries=3` pour apt, et les appels apt et dnf de `release.yml` passent par `scripts/ci-retry-network.sh`.
- [ ] Un test de script lance `scripts/ci-retry-network.sh` contre une commande factice qui émet chaque motif transitoire puis réussit. Il vérifie que le motif est retenté, et qu'une erreur non transitoire ne l'est pas.
- [ ] Given un step bloqué, then le job est tué à son timeout et l'agrégateur rapporte l'échec.

---

### EP-005: Une chaîne d'approvisionnement et une publication vérifiées

Phase 2. Les actions sont à jour et surveillées, et ce qui est publié est vérifié contre la signature de la release.

**Definition of Done:**
- 0 finding zizmor de sévérité haute non justifié.
- Plus aucune annotation « forced to run on Node.js 24 ».
- Dependabot propose les mises à jour d'actions.
- apt, rpm et le cask ne publient que des assets dont la signature minisign est vérifiée.
- Les assets de release portent une attestation de provenance.

#### US-018: Des actions à jour, épinglées et surveillées
**Description:** As a mainteneur, I want que toutes les actions tournent sur leur runtime natif, soient épinglées par SHA de commit et soient surveillées so that une action vulnérable ou abandonnée soit signalée au lieu d'être découverte (R-12, T-14, D-13, L-16).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] `actions/upload-artifact`, `actions/download-artifact` et `softprops/action-gh-release` passent sur leurs majors Node 24 (v7, v8, v3 au 2026-11-02), épinglés par SHA avec un commentaire de version exact. `ilammy/msvc-dev-cmd` est remplacé par un step de script ou justifié.
- [ ] Given les logs des runs suivants, then aucune annotation « target Node.js 20 but are being forced to run on Node.js 24 » n'apparaît.
- [ ] `scripts/check-workflows.sh` scanne chaque workflow et chaque action composite, `libghostty-bump.yml` compris. Il rejette les refs de tag (les 6 `download-artifact@v4`) et les SHA d'objets tag annotés (`Swatinem/rust-cache@42dc69e`, `nathanvaughn/actions-cloudflare-purge@1856dd6`).
- [ ] Given une PR qui ajoute `uses: foo/bar@v1`, when le checker s'exécute, then il échoue en nommant `fichier:ligne`.
- [ ] `.github/dependabot.yml` couvre l'écosystème `github-actions` : hebdomadaire, groupé, `cooldown` de 7 jours.
- [ ] zizmor, en version épinglée, tourne quand `.github/**` change et échoue sur un finding de sévérité haute. Les exceptions justifiées sont dans `.github/zizmor.yml` (images `opensuse/tumbleweed:latest`, déclencheurs `workflow_run` gardés).

#### US-019: La publication vérifie ce qu'elle publie
**Description:** As a utilisateur Linux ou macOS, I want que les dépôts apt et rpm et le cask ne publient qu'un asset vérifié et la bonne version so that un asset remplacé ou un rendu raté n'atteigne jamais mon gestionnaire de paquets (D-03, D-05, D-06, D-15).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] `repo_publish.yml` vérifie la signature `.minisig` de chaque asset contre la clé publique embarquée avant de signer en GPG les métadonnées apt et rpm. Given une signature invalide, then le job échoue avant toute écriture R2.
- [ ] `update_cask.yml` checkout `github.event.workflow_run.head_sha` et rend le cask depuis le template du tag. Given un `sed` qui laisse `0.0.0` ou une version différente du tag, then le job échoue avant le push.
- [ ] `update_cask.yml` lance `ruby -c` sur le cask rendu et refuse une version inférieure à celle du tap.
- [ ] `scripts/verify-published-streams.sh` vérifie la version dans apt `Packages`, rpm `repodata`, le cask et la validité des signatures. Il contourne le cache CDN (requête avec cache-busting) et sert à la fois à `repo_publish.yml:584-676` et à l'étape 5 du runbook.
- [ ] Given `Packages.gz` montre encore l'ancienne version après 5 tentatives espacées de 60 s, then la vérification échoue en nommant le flux.

#### US-020: Les assets de release sont attestés et immuables
**Description:** As a utilisateur, I want que chaque asset de release porte une attestation de provenance et ne puisse plus être remplacé après publication so that je puisse vérifier d'où vient le binaire que j'installe (R-11, D-05).

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] La publication crée un draft, uploade les assets, compare le `digest` de chaque asset (API releases) au sha256 local, attache une attestation `actions/attest-build-provenance`, puis publie.
- [ ] Given un écart de digest, then la publication échoue et le draft reste en place pour inspection.
- [ ] Given un rerun après publication, then il échoue avec un message, au lieu d'écraser les assets (`release.yml:1847-1886`).
- [ ] Le runbook contient la commande pour activer les releases immuables, qu'Arthur applique. La procédure de re-tag d'AGENTS.md devient : supprimer et recréer le tag seulement avant publication, et couper une version patch après.
- [ ] `docs/user/installation/` documente `gh attestation verify` pour chaque plateforme.

---

### EP-006: Une structure simple et des runners pérennes

Phase 2. La CI quitte les images Ubuntu 22.04 avant leurs brownouts, sans perdre le plancher glibc 2.35, et ses deux gros workflows deviennent lisibles.

**Definition of Done:**
- 0 `runs-on` sur `ubuntu-22.04` ou `ubuntu-22.04-arm` avant le 2027-03-23, et le plus haut symbole `GLIBC_` des binaires Linux reste au plus à 2.35.
- `release.yml` tient en 400 lignes au plus.
- Un seul workflow de recette libghostty existe.
- Les deux spikes sont supprimés.

#### US-021: Validate assumption: le build Linux de release tourne dans `ubuntu:22.04` sur un hôte 24.04
**Description:** As a mainteneur, I want prouver que les legs Linux x86_64 et aarch64 buildent dans un conteneur `ubuntu:22.04` sur `ubuntu-24.04` et `ubuntu-24.04-arm` so that la migration de US-022 garde le plancher « Ubuntu 22.04+ » de `packaging/debian/postinst:29`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Sur un run de branche, les deux legs buildent `paneflow-app` et `paneflow-host` en release dans `container: ubuntu:22.04`, avec les dépendances de `.github/actions/linux-build-deps`.
- [ ] `objdump -T` sur chaque binaire ne montre aucun symbole `GLIBC_` supérieur à 2.35.
- [ ] Le `.deb` produit passe le smoke existant dans `ubuntu:22.04`, et les plafonds de taille des helpers tiennent.
- [ ] Le temps de build en conteneur est consigné, avec au plus 15 % d'écart par rapport au build sur l'hôte.
- [ ] Given une dépendance qui échoue dans le conteneur, then l'échec est consigné et le repli est retenu : plancher relevé à 24.04 (glibc 2.39), postinst et `docs/user/installation/linux.md` mis à jour, note de release.

#### US-022: Plus aucun job sur les images Ubuntu 22.04
**Description:** As a mainteneur, I want que chaque job quitte les images 22.04 avant le premier brownout so that la CI ne s'arrête pas le 2027-03-23.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-021

**Acceptance Criteria:**
- [ ] Les 42 lignes `runs-on: ubuntu-22.04` ou `ubuntu-22.04-arm` et les deux legs de matrice de `release.yml:143,150` passent sur `ubuntu-24.04` ou `ubuntu-24.04-arm`. Les builds Linux de release tournent dans le conteneur validé par US-021.
- [ ] Les 4 `ubuntu-latest` deviennent des labels explicites.
- [ ] Un gate de release échoue si un binaire Linux référence un symbole `GLIBC_` supérieur au plancher retenu par US-021. Given un binaire qui exige `GLIBC_2.36` avec un plancher à 2.35, then le gate échoue en nommant le symbole.
- [ ] `scripts/check-workflows.sh` échoue sur toute ligne `runs-on` qui vise une image dépréciée (`ubuntu-22.04`, `ubuntu-22.04-arm`, `macos-14`).
- [ ] La migration est livrée avant le 2027-03-23.

#### US-023: Les spikes et le code mort disparaissent
**Description:** As a mainteneur, I want supprimer les workflows de spike et les étapes mortes so that la CI ne contienne que ce qui sert.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `gungraun-spike.yml` et `scripts/gungraun-spike.sh`, dont la logique a été absorbée par `scripts/perf-ab.sh:203-296`, sont supprimés, ainsi que `perf-desktop-spike.yml` et `scripts/perf-desktop-spike.sh`, dont la configuration a été promue dans `perf_gates`.
- [ ] `bench/README.md` cite les run IDs historiques (37369490241, 37198169856) à la place des workflows. La raison du `#[ignore]` de `desktop_headless_spike` (`crates/paneflow-host/tests/persistent_baseline.rs:858`) pointe vers `bench/README.md`.
- [ ] Les étapes signtool mortes et le `cargo check` redondant après clippy (T-11) sont retirés de `run_tests.yml`.
- [ ] Given un `grep -r "gungraun-spike\|perf-desktop-spike"` sur le dépôt, then il ne renvoie aucun résultat hors de `CHANGELOG.md`.
- [ ] `scripts/check-workflows.sh` vérifie que chaque `scripts/*` cité par un workflow ou une action composite existe. Given un workflow qui appelle encore un script supprimé, then il échoue en nommant le workflow et le script absent.

#### US-024: Les scripts inline de `release.yml` deviennent des scripts versionnés
**Description:** As a mainteneur, I want sortir les blocs de script inline de `release.yml` dans `scripts/` so that chaque étape de release se teste en local et que le workflow redevienne lisible (R-13, première partie).

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-016

**Acceptance Criteria:**
- [ ] Le smoke MSI (452 lignes de pwsh inline), le smoke d'upgrade avec host vivant, la signature des paquets Linux (`:663-906`), la mise en place des assets (`:1617-1683`), celle des helpers Windows (`:420-484`) et l'installation de minisign deviennent des scripts sous `scripts/`, sans commentaire, chacun lançable en local avec des arguments documentés dans `docs/release/runbook.md`.
- [ ] Les scripts PowerShell posent `$ErrorActionPreference = 'Stop'` et vérifient `$LASTEXITCODE` après chaque commande native. Les scripts bash utilisent `set -euo pipefail`.
- [ ] Given une étape de script qui échoue, then le step du workflow échoue avec le code de sortie du script.
- [ ] `release.yml` perd au moins 1 500 lignes, et un dry-run produit exactement les mêmes noms d'assets et les mêmes vérifications que le dernier dry-run précédent.

#### US-025: `release.yml` devient un orchestrateur de workflows réutilisables
**Description:** As a mainteneur, I want un orchestrateur court qui appelle un workflow réutilisable par plateforme so that chaque plateforme reçoive seulement ses secrets, se relance seule et ne porte plus de branches `if: runner.os` (R-13, seconde partie).

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-024, US-007

**Acceptance Criteria:**
- [ ] `release.yml` tient en 400 lignes au plus et contient `preflight`, `ci-verdict`, trois appels `workflow_call` (Linux, macOS, Windows), une matrice de smoke Linux par distribution et la publication.
- [ ] Chaque workflow de plateforme reçoit ses secrets par un mapping `secrets:` explicite, jamais `secrets: inherit`, et déclare ses propres `permissions` et `timeout-minutes`.
- [ ] Aucun workflow de plateforme ne contient de condition `if: runner.os`.
- [ ] Given un échec sur la seule plateforme Windows, when Arthur utilise « Re-run failed jobs », then seul le workflow Windows est relancé.
- [ ] Given un workflow appelé qui demande plus de permissions que l'appelant, then le run échoue à la validation, et la PR le montre sur un run de branche.

#### US-026: Un seul workflow de recette libghostty
**Description:** As a mainteneur, I want un workflow réutilisable `libghostty-recipe.yml`, appelé par le nightly, par les changements de recette et par le bump, so that la double reconstruction et la comparaison au manifest n'existent qu'en un exemplaire.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006, US-015

**Acceptance Criteria:**
- [ ] `libghostty-recipe.yml` (`workflow_call`) reconstruit deux fois sur Linux x86_64, Linux aarch64 et en cross macOS. Il compare avec `archive_sha256` du manifest et avec l'asset publié, et prend une entrée `allow_hash_drift` et un artifact de manifest candidat optionnel.
- [ ] `libghostty-linux.yml` et `libghostty-macos.yml` perdent leurs jobs `native` en double, et les jobs `linux` et `macos` de `libghostty-bump.yml` (`:285-415`) appellent le workflow de recette.
- [ ] Given un hash qui dérive sans `allow_hash_drift`, then le workflow de recette échoue en nommant la cible et les deux hashes.
- [ ] Les contextes requis du ruleset `main: required CI` sont mis à jour dans le même changement. Arthur applique la commande `gh api` fournie, et une PR sans effet sur le code reste fusionnable.

## Functional Requirements

- FR-01: Le système ne doit publier une release que si chaque job requis de `run_tests` est `success` sur le SHA exact de la release.
- FR-02: Le système doit résoudre le SHA de la release une seule fois et construire chaque job de la release à partir de ce SHA.
- FR-03: Quand un fichier lu par le build change, le système doit lancer clippy et les tests des quatre plateformes.
- FR-04: Le système doit lire les pins libghostty uniquement depuis `native/libghostty/manifest.toml`.
- FR-05: Quand un test échoue puis passe, le système doit le nommer dans une annotation et dans le JUnit, et faire échouer le run sauf si le test est en quarantaine.
- FR-06: Quand une advisory touche `main` ou la dernière release, le système doit ouvrir ou commenter une issue `type: security`.
- FR-07: Le système ne doit écrire aucun secret dans un log, masqué ou non.
- FR-08: Le système ne doit servir les secrets de signature et de publication qu'aux jobs de l'environnement `release`, sur `main` ou un tag `v*`.
- FR-09: Le système ne doit publier dans apt, rpm ou le cask qu'un asset dont la signature minisign est valide.
- FR-10: Le système ne doit pas sauvegarder de cache depuis une PR ou un tag, ni utiliser de cache dans `release.yml`.
- FR-11: Le système ne doit pas relancer une suite entière pour masquer un échec.
- FR-12: Le système doit vérifier mécaniquement, dans `scripts/check-workflows.sh`, les pins SHA, l'absence de `-x`, la présence de `timeout-minutes`, le `save-if` de chaque `rust-cache`, l'absence de `GHOSTTY_SHA` littéral, l'absence d'image de runner dépréciée et l'existence de chaque script cité.

## Non-Functional Requirements

- **Performance:** chemin critique d'une PR qui touche du Rust en 25 min au plus (28,8 min aujourd'hui pour le leg Windows) ; release dry-run en 30 min au plus (48,9 min) ; `ci-verdict` ajoute au plus 2 min quand `run_tests` est déjà terminé.
- **Security:** 0 secret dans un log, mesuré par 0 ligne `***` issue d'un secret multi-ligne dans les logs de `repo_publish` et `release` ; 0 finding zizmor de sévérité haute non justifié (13 aujourd'hui) ; 100 % des actions épinglées sur un SHA de commit ; secrets de signature servis à 0 job hors de l'environnement `release`.
- **Reliability:** au plus 2 retries par test en quarantaine ; hors quarantaine, un seul rerun du test en échec, qui sert à le nommer FLAKY et ne le fait jamais passer ; quarantaine de 4 tests au plus au départ et de 2 au plus après 6 mois ; timeout par test de 300 s (60 s × 5) ; chaque job déclare un `timeout-minutes` d'au plus 1,5 fois sa durée maximale observée ; 0 nightly libghostty annulé par un push (17 sur 19 aujourd'hui).
- **Scalability:** cache Actions sous 8 GB pour un quota de 10 GB (11,24 GB aujourd'hui) ; au moins 80 % des jobs `run_tests` sur `main` restaurent un cache.
- **Accessibility:** non applicable, aucune interface utilisateur. Les annotations et résumés d'étape nomment le test, le fichier et la commande de reprise en texte brut.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Verdict pas encore disponible | Tag poussé pendant que `run_tests` tourne sur le SHA | `ci-verdict` interroge toutes les 60 s pendant 60 min au plus | "Waiting for run_tests on <sha>: <n> of <m> required jobs done" |
| 2 | Job requis sauté | Tag sur un commit dont les jobs Rust ont été filtrés | `ci-verdict` échoue, sans publication | "Required job <name> was skipped on <sha>. Run: gh workflow run run_tests.yml --ref <tag>, then re-run this release" |
| 3 | Tag déplacé pendant le run | Tag recréé sur un autre commit (procédure AGENTS.md) | La publication échoue avant tout upload | "Tag <tag> now points to <sha2>, this run built <sha1>. Re-run the release" |
| 4 | Dry-run depuis une branche | `workflow_dispatch` sur une branche non autorisée | Build non signé, pas de publication, pas d'échec sur la policy | "Signing secrets unavailable on <ref>: building unsigned dry-run artifacts" |
| 5 | Signature Windows ratée | `sign-windows.ps1` sort en non-zéro, service de signature indisponible | Le step échoue, rien n'est uploadé | "Signing failed for <file> (exit <code>)" |
| 6 | Flaky hors quarantaine | Un test échoue puis passe sous nextest | Le job échoue, annotation `::error` | "<test> is FLAKY: failed then passed. Fix it or add it to the quarantine with an issue" |
| 7 | Test en quarantaine qui échoue 3 fois | Une régression réelle sur un test en quarantaine | Le job échoue | "<test> failed all 3 attempts" |
| 8 | Test bloqué | Une session ConPTY ne se termine jamais | nextest tue le test après 300 s, job object compris sous Windows | "<test> TIMEOUT after 300 s" |
| 9 | Alerte non déposée | Label absent ou API GitHub indisponible pendant `gh issue create` | Le job d'audit échoue | "Security alert was NOT filed: <gh error>" |
| 10 | API GitHub dégradée | Rate limit ou erreur 5xx pendant le polling de `ci-verdict` | Retries avec backoff, puis échec fermé à 60 min | "Could not read check runs for <sha>: <error>" |
| 11 | Erreur réseau transitoire | `CRYPT_E_REVOCATION_OFFLINE`, Zig `ReadFailed`, apt 5xx | Retry, jusqu'à 5 tentatives pour curl | None |
| 12 | Asset de release remplacé | Asset modifié après publication, release mutable | `repo_publish` échoue sur minisign avant toute écriture R2 | "Signature check failed for <asset>: not publishing" |
| 13 | Rendu du cask raté | Le template change et le `sed` ne matche plus | `update_cask` échoue avant le push | "Cask version is <found>, expected <tag>" |
| 14 | Quota de cache dépassé | Nouveaux jobs ou nouvelles clés | Éviction LRU ; les seules sauvegardes venant de `main`, le total reste sous 8 GB | None |
| 15 | Deux releases rapprochées | Deux tags poussés à moins d'une minute | `queue: max` les sérialise sans en remplacer | None |
| 16 | Bump sur une branche existante | La branche de bump existe déjà | Échec avant de publier la release et l'attestation | "<branch> already exists. Delete it, or finish the bump that is already open." |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Exiger un macOS vert bloque les releases tant que le leg échoue sur des flaky dans 35 % des exécutions | High | High | US-007 est bloquée par US-012 ; US-010 quarantaine les flaky résiduels, avec une issue chacun ; la commande de reprise est donnée par `ci-verdict` |
| 2 | Le processus par test de nextest casse des tests qui partagent un état | Med | Med | Spike US-009 sur les quatre legs, 5 runs chacun. Les mutations d'env connues (`crates/paneflow-serve/tests/*`) deviennent plus sûres |
| 3 | Le conteneur `ubuntu:22.04` ne suffit pas au build GPUI et libghostty | Med | Med | Spike US-021 avant US-022 ; repli défini (plancher 2.39, doc et note de release) ; échéance du 2027-03-23 suivie |
| 4 | Un renommage de check requis bloque les merges | Med | High | Les agrégateurs `tests_pass` et `libghostty validation` gardent leur nom ; le ruleset est mis à jour dans le même changement (US-026) |
| 5 | La deployment policy bloque une release légitime | Low | High | La policy autorise `main` et `v*` ; un dispatch de test depuis une branche et un dry-run sur `main` précèdent la suppression des secrets du dépôt (US-004) |
| 6 | Le bruit des PR Dependabot | Med | Low | Mises à jour groupées, hebdomadaires, avec un `cooldown` de 7 jours |
| 7 | Les faux positifs de zizmor | Med | Low | Exceptions justifiées dans `.github/zizmor.yml` ; seule la sévérité haute bloque |
| 8 | Les releases immuables contredisent la procédure de re-tag | High | Med | Flux draft d'abord ; AGENTS.md réécrit (US-020) ; activation par Arthur après livraison du flux |
| 9 | Les changements PowerShell ne se vérifient que sur Windows | High | Med | Chaque story Windows nomme le runner réel ou l'hôte Windows d'Arthur ; dry-run de `release.yml` avant tout tag |
| 10 | Une session concurrente pousse sur le même checkout | Med | Med | Une story par commit ; le jeu indexé est vérifié avant chaque push (pratique du dépôt) |
| 11 | Sans release réelle, les secrets que seule la publication consomme (`R2_*`, `CLOUDFLARE_*`, `HOMEBREW_TAP_DEPLOY_KEY`, `MINISIGN_SECRET_KEY`) ne sont pas exercés avant la première release après ce PRD | Med | Med | `configure-release-environment.sh status` prouve leur présence dans `release` ; le dry-run sur `main` exerce Apple, Azure et GPG ; la première release est surveillée, et `repo_publish` ou `update_cask` se relancent par `workflow_dispatch` après correction |

## Non-Goals

- **Runners self-hosted ou larger runners** (approche de Zed) : ils sont facturés même pour un dépôt public, et les runners standard suffisent une fois les doublons retirés.
- **sccache avec un backend R2 ou GHA** : sccache ne cache pas le link du crate `bin` GPUI, et il multiplie les entrées face à la limite de 200 uploads par minute. À revoir après US-014 si le cache reste insuffisant.
- **Lockfile d'actions (`gh actions-lock`)** : GA le 2026-10-28, il a 5 jours. Pins SHA, Dependabot et zizmor couvrent le besoin en v1. À revoir au premier trimestre 2027.
- **OIDC pour Azure Trusted Signing** : il demande une configuration Azure externe. L'environnement protégé (US-004) réduit déjà l'exposition. À revoir après US-004.
- **Matrice unique pour les jobs de plateforme de `run_tests`** (T-12) : à revoir après la stabilisation de nextest.
- **Refonte du coût de `perf-ab`** (D-10) et **purge du pool RPM** (D-16) : hors du périmètre de fiabilité de ce PRD.
- **Rendre macOS non bloquant** : 19 vrais échecs de la fenêtre n'ont été détectés que sur macOS. La quarantaine par test remplace cette option.
- **Merge queue** : le flux principal est le push direct sur `main`.

## Files NOT to Modify

- `crates/paneflow-host/src/runtime.rs`, `crates/paneflow-host/src/host.rs` : le contrat de libération de la tail et du record complété (`ARCHITECTURE.md:935-951`), dont dépendent les documents de qualification.
- `crates/paneflow-host/src/server.rs` hors de `mod tests` : US-011 reste dans le module de tests.
- `src-app/build.rs` : construction des helpers `release-min` et plafond total de taille ; US-015 lit ses sorties sans le modifier.
- `native/libghostty/manifest.toml` et `native/libghostty/prebuilt/**` : seul le bump les modifie.
- Les quatre `rev` GPUI de `src-app/Cargo.toml` et `rust-toolchain.toml`.
- `packaging/paneflow-release.asc` et `keys/` : clés publiques de signature.
- `deny.toml`, sauf si une advisory l'exige.

## Technical Considerations

- **Architecture :** `ci-verdict` lit les check runs du SHA par l'API (recommandé) plutôt que de déclencher la release sur `workflow_run` de `run_tests`. L'ingénierie doit confirmer que les noms de check runs sont stables entre tentatives et que l'API rend la dernière tentative de chaque job.
- **Environnement et dry-run :** l'expression d'`environment:` qui vaut `release` sur `main` et `v*`, et rien ailleurs, se comporte-t-elle comme attendu (pas d'environnement, donc pas d'échec de policy) ? Sinon, un job de signature séparé, conditionné par la ref.
- **nextest et conpty :** convertir `tests/conpty.rs` en `harness = true` (recommandé, un seul runner) ou l'exclure et garder un step `cargo test --test conpty` ? Le spike US-009 tranche.
- **Lecteur du manifest :** bash avec un parseur TOML minimal, ou Python `tomllib` (présent sur les runners) ? Recommandé : `tomllib`, avec un fallback bash pour les runners Windows sans Python dans le PATH. L'ingénierie doit confirmer la présence de Python 3.11 sur `windows-2022`.
- **Smoke indépendant des tests (US-013) :** les outputs d'un job en échec sont-ils lisibles par un job dépendant `if: ${{ !cancelled() }}` ? Sinon, séparer build et tests en deux jobs.
- **Dépendances :** cargo-nextest (installé par `taiki-e/install-action`) et zizmor (version épinglée, installé par `pipx` ou `uvx`) sont de l'outillage CI, pas des dépendances du produit. Aucune nouvelle crate.
- **Migration :** chaque story se défait par un revert de son commit. Le passage des secrets dans l'environnement (US-004) garde les copies du dépôt jusqu'à un dry-run de `release.yml` sur `main` qui signe chaque leg avec les secrets de l'environnement ; aucune release réelle n'a lieu pendant ce PRD.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Part des runs `run_tests` en échec qui ne portent que des flaky | 45,8 % (44 sur 96) | Inférieure à 15 % | Month-1 après EP-003 | Classification de `flaky-forensics.md` rejouée sur 5 semaines, ou JUnit FLAKY agrégé |
| Jobs Windows verts passés seulement sur retry caché | 39,5 % (51 sur 129) | 0 retry caché ; flaky visibles en annotation | Month-1 | Annotations et JUnit `flakyFailure` |
| Releases publiées sur un SHA rouge | 1 (v0.19.0) | 0 | Month-1 | `ci-verdict` dans chaque run de release |
| Advisories notifiées | 0 sur 34 | 100 % en moins de 24 h | Month-1 | Issues `type: security` comparées aux échecs de `audit.yml` |
| Jobs `run_tests` sur `main` qui restaurent un cache | Environ 0 % (aucune entrée) | Au moins 80 % | Month-1 après US-014 | « Cache restored » dans les logs ; `gh api .../actions/caches` |
| Taille du cache Actions | 11,24 GB | Sous 8 GB | Month-6 | `gh api .../actions/cache/usage` |
| Durée d'une release | 48,9 min | 30 min au plus | Month-6 | Timestamps des jobs du run de release |
| Findings zizmor de sévérité haute | 13 | 0 non justifié | Month-6 | zizmor en CI (US-018) |
| Runs sur images Ubuntu 22.04 | 42 lignes et 2 legs | 0 | Avant le 2027-03-23 | `scripts/check-workflows.sh` |
| Nightlies libghostty annulés par un push | 17 sur 19 | 0 | Month-1 après US-008 | `gh run list --event schedule` |
| Tests en quarantaine | N/A (new) | 4 au plus, puis 2 au plus | Month-1, puis Month-6 | `.config/nextest.toml` |

## Open Questions

- Le secret `GPG_PASSPHRASE` ne contient que des espaces (run 41428495013). La clé des dépôts est-elle volontairement sans passphrase ? Arthur, avant US-002 : la réponse décide si le contrôle de présence doit refuser une valeur vide après trim.
- La PR #110 du bot vise le pin déjà actif. Faut-il la fermer ? Arthur, avant US-006 ; aucune fermeture n'a lieu sans son accord.
- Les dry-runs depuis une branche doivent-ils pouvoir signer (par exemple pour qualifier un MSI signé avant merge) ? Arthur, avant US-004 : si oui, il faut une policy plus large ou un environnement `release-dry-run` avec réviseur.
- Les jobs agrégateurs peuvent-ils passer sur `ubuntu-slim` (1 vCPU, timeout de 15 min) ? L'ingénierie, pendant US-022 ; cela ne bloque rien.
- Quel modèle de runner macOS les perf gates doivent-elles viser pour le SLA de 600 ms déplacé par US-012 ? L'ingénierie, pendant US-012 : il faut une classe de machine calibrée, sinon le SLA reste sous Linux seulement.
[/PRD]

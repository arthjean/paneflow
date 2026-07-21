# Audit final OpenAI Build Week: Paneflow

Audit effectué le 21 juillet 2026 sur le dépôt public au commit `690c960`, la soumission Devpost publiée et les données officielles du hackathon. Aucun test ni build local n'a été lancé. Les validations citées ci-dessous sont celles des pipelines publics.

## Verdict

**Paneflow est éligible et compétitif. Je ne vois aucun motif évident de disqualification.** L'extension réalisée pendant la Build Week est incontestablement significative, le projet fonctionne sous forme de releases natives, la vidéo respecte la durée et couvre bien Paneflow, Codex et GPT-5.6, le dépôt est public et licencié.

Les incohérences documentaires relevées pendant l'audit ont été corrigées dans la documentation: `BUILD_WEEK.md` est synchronisé avec la version finale, le README couvre v0.8.1 et le port Windows avec un parcours de test juge, et `ARCHITECTURE.md` ainsi que le haut du `CHANGELOG.md` décrivent désormais l'état livré. Le risque restant est opérationnel: conserver le dépôt, la release, le site et la vidéo accessibles pendant toute la période de jugement.

## Conformité au règlement

| Exigence | Verdict | Preuves et réserve |
|---|---|---|
| Soumission dans la fenêtre | Conforme | Devpost indique Paneflow `published` et soumis avant la date limite, avec une dernière mise à jour le 21 juillet. La deadline officielle est le 21 juillet à 17:00 PT, soit le 22 juillet à 02:00 CEST. |
| Eligibilité | Conforme sous réserve des données de profil | La France est éligible et la règle exige d'avoir atteint l'âge légal de majorité. La soumission est enregistrée comme projet de l'auteur. |
| Projet fonctionnel | Conforme | Release publique [v0.8.1](https://github.com/arthjean/paneflow/releases/tag/v0.8.1), artefacts Linux x86_64/aarch64, macOS Apple Silicon et Windows x64, avec checksums et signatures. Le pipeline de release du SHA `690c960` est terminé avec succès. |
| Projet préexistant significativement étendu après le 13 juillet à 09:00 PT | Conforme, preuve très forte | Baseline [v0.7.10](https://github.com/arthjean/paneflow/releases/tag/v0.7.10), commit `e82b3da`, publié le 10 juillet. `e82b3da..690c960` contient 139 commits, tous datés après l'ouverture, 368 fichiers modifiés, 58 012 insertions et 20 363 suppressions. Le [compare public](https://github.com/arthjean/paneflow/compare/v0.7.10...v0.8.1) isole le travail évalué. |
| Documentation distinguant ancien et nouveau | Conforme | Devpost, `BUILD_WEEK.md` et le README donnent désormais la baseline v0.7.10, les releases livrées et le compare complet jusqu'à v0.8.1. |
| Usage de Codex et GPT-5.6 pendant la fenêtre | Conforme | La soumission, la vidéo et le README documentent l'usage. Les commits et PRD sont datés. Le champ privé `/feedback` est rempli et sa valeur a été confirmée par Arthur. |
| Catégorie | Conforme | La capture du formulaire confirme `Developer Tools`. |
| Démo YouTube | Conforme | [Vidéo actuelle](https://youtu.be/pSdQbGpO8cI) publique (`isUnlisted=false`), 134 secondes, avec piste audio muxée et sous-titres automatiques anglais. Arthur a confirmé la version finale après contrôle. |
| Dépôt et licence | Conforme | [Dépôt public](https://github.com/arthjean/paneflow), GPL-3.0 selon GitHub, `Cargo.toml:28` en `GPL-3.0-or-later`, fichier `LICENSE`, notices Ghostty et SBOM dans `native/libghostty/`. |
| README: installation, plateformes, exécution | Conforme | Les sections [Install](../README.md#install) et [Build from source](../README.md#build-from-source) couvrent Linux, macOS et Windows, avec des paquets prêts à lancer et les instructions de compilation. |
| README: collaboration Codex, GPT-5.6 et décisions humaines | Conforme | La section Build Week contient maintenant la baseline, les contraintes décidées par Arthur, le rôle concret de Codex et GPT-5.6, les validations, Windows livré et les liens de preuve. |
| Dev tool: chemin de test sans rebuild | Conforme | Le README contient un bloc `Judge quick test` basé sur les paquets v0.8.1 et le rollback Alacritty. La capture du formulaire confirme que le champ privé d'installation et de test est rempli. |
| Langue | Conforme | Soumission, README et narration vidéo en anglais. |
| Captures Devpost | Risque faible accepté | Les quatre captures sont visuellement solides. Une partie du texte produit visible reste en français, mais Arthur considère ce détail non bloquant et ne souhaite pas remplacer les captures. |
| Propriété intellectuelle | Aucun écart visible dans le dépôt | GPL, licences des dépendances natives, `THIRD_PARTY_NOTICES.md` et SBOM sont présents. L'absence de musique ou d'éléments vidéo non autorisés n'a pas été contrôlée visuellement dans cet audit de dépôt. |

Les clauses déterminantes des règles officielles sont explicites: "Pre-existing Projects will be evaluated only on work added during the Submission Period" et "must have been meaningfully extended using Codex and/or GPT-5.6 after the Submission Period start date". Paneflow répond largement à ces deux points.

## Exactitude des claims techniques

| Claim de la soumission | Verdict | Preuve |
|---|---|---|
| `e82b3da` / v0.7.10 comme baseline | Exact | Tag annoté v0.7.10 sur `e82b3da`, daté du 10 juillet, antérieur au début officiel du 13 juillet. |
| v0.7.11, v0.8.0 et v0.8.1 livrées pendant l'événement | Exact | Releases publiques des 14, 18 et 21 juillet. |
| Ghostty par défaut sur Linux et Windows x64 MSVC, Alacritty sur macOS et en rollback | Exact | `src-app/Cargo.toml:15-35`, `crates/paneflow-config/src/schema.rs:567-576`, `src-app/src/terminal/view.rs:70-104`. |
| Corpus différentiel de 135 cas | Exact | `src-app/src/terminal/backend_corpus.rs:18-20`: 27 familles x 5 variantes. |
| Deux cibles de fuzzing | Exact | `fuzz/fuzz_targets/parser.rs` et `fuzz/fuzz_targets/snapshot.rs`. |
| Archives statiques épinglées et reproductibles pour Linux x86_64/aarch64 et Windows x64 | Exact | `native/libghostty/manifest.toml`, les trois arbres `native/libghostty/prebuilt/`, les scripts Linux/Windows et les workflows dédiés. |
| Cargo ne télécharge ni ne compile Ghostty | Exact pour le build normal | `native/libghostty/README.md:1-10` et `crates/paneflow-libghostty-sys/build.rs:545-552`. La reconstruction des archives reste un pipeline séparé avec Zig. |
| ConPTY fermé sur un thread dédié avec drain final | Exact | `src-app/src/terminal/ghostty_session.rs:862-914` puis le cycle de drain autour de `2323-2430`. |
| Release Windows x64 avec provenance et MSI signé | Exact au niveau CI/release | [v0.8.1](https://github.com/arthjean/paneflow/releases/tag/v0.8.1) contient le MSI et le rapport `libghostty-provenance.json`; les workflows release, tests, libghostty Linux et libghostty Windows du SHA final sont au vert. |
| Qualification manuelle Windows 10/11 entièrement archivée | Non revendiquée sur Devpost, mais lacune interne à connaître | `tasks/prd-windows-libghostty-backend-2026-Q3-status.json:6` reste `IN_PROGRESS`; les entrées Windows 10 et Windows 11 portent `OWNER_RISK_ACCEPTANCE` et `evidence_status: NOT_COLLECTED`. Cela ne contredit pas la release, mais il ne faut pas transformer la qualification CI en preuve d'une matrice desktop manuelle complète. |

## Lecture par critère du jury

| Critère | Avis | Pourquoi |
|---|---|---|
| Technological Implementation | Très fort | C'est le meilleur axe: migration FFI/PTY réelle, isolation de l'unsafe, corpus différentiel, fuzzing, supply chain native, ConPTY, CI et releases. Le travail est non trivial, livré et désormais documenté de façon cohérente dans le README. |
| Design | Fort | Ce n'est pas un prototype technique: application native distribuée, workflows cohérents, Review, navigation, états des agents et site public répondant en HTTP 200. Les quatre captures sont visuellement fortes; le texte français visible sur certaines reste une friction mineure acceptée. |
| Potential Impact | Fort | Le problème est précis et crédible: superviser plusieurs agents, leurs attentes, branches, erreurs et diffs. Le dogfooding de Paneflow pour construire Paneflow rend le cas concret. Les 35 stars et 4 forks sont un premier signal réel, encore trop tôt pour constituer une preuve forte d'adoption. |
| Quality of the Idea | Bon à fort | Le marché des workspaces agentiques est déjà actif. La différenciation crédible est l'ensemble natif et local: vrais PTY, états agent, orchestration, worktrees, Review et MCP read-only. Le jury doit retenir ce système complet, pas seulement "Alacritty remplacé par Ghostty". |

## Etat des actions

1. **Fait:** `BUILD_WEEK.md` est synchronisé avec la version finale rédigée par Arthur.
2. **Fait:** le README expose directement la baseline, le compare, le rôle de Codex et GPT-5.6, les décisions humaines, Windows livré et macOS sur Alacritty.
3. **Fait:** le README contient un parcours de test juge sans reconstruction.
4. **Fait:** les captures du formulaire confirment `Submitted`, `Individual`, `France`, `Developer Tools`, le dépôt public, le `/feedback` et les instructions privées. Arthur confirme que ces valeurs sont correctes.
5. **Fait:** `ARCHITECTURE.md` et `CHANGELOG.md` reflètent le backend dual et v0.8.1.
6. **Décision:** les captures contenant du français restent en place. Arthur accepte ce risque faible.
7. **Garde-fou:** ne pas revendiquer une matrice manuelle Windows 10/11 entièrement archivée. Le claim vérifié porte sur le backend Windows x64 livré, la CI et le pipeline MSI.
8. **Garde-fou:** après la deadline, ne plus modifier les éléments soumis sans accord des organisateurs et maintenir toutes les ressources accessibles pendant le jugement.

## Sources principales

- [Règles officielles](https://openai.devpost.com/rules)
- [Soumission Paneflow](https://devpost.com/software/paneflow)
- [Dépôt public](https://github.com/arthjean/paneflow)
- [Comparaison Build Week](https://github.com/arthjean/paneflow/compare/v0.7.10...v0.8.1)
- [Release v0.8.1](https://github.com/arthjean/paneflow/releases/tag/v0.8.1)
- [Pipeline de release v0.8.1](https://github.com/arthjean/paneflow/actions/runs/29806399813)
- [Vidéo de démonstration](https://youtu.be/pSdQbGpO8cI)

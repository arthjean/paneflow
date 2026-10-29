[PRD]
# PRD: Re-pin libghostty-vt sur Ghostty v1.4.0 et passage à Zig 0.17.0

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-28 | Arthur Jean | PRD initial : re-pin de libghostty-vt de `b699ea79` (1.3.2-dev, Zig 0.16.0) vers le tag Ghostty `v1.4.0` (`83fb9562`, Zig 0.17.0). Adaptation des recettes à la CLI de Zig 0.17, portage de `vt_write_until_ground`, tests des trois correctifs de sécurité traversés, requalification Linux et macOS, Windows sur matériel réel en dernier epic. 4 epics, 14 stories. |
| 1.1 | 2026-10-28 | Arthur Jean | Tout le travail Windows passe dans EP-004 : analyse de l'archive et de la recette Windows, jobs Windows de la CI, passe matérielle. Hors d'EP-004, il ne reste que les deux prérequis du run de bump (valeurs Zig Windows du manifeste, options de la recette PowerShell), faits depuis Fedora et vérifiés par la CI. EP-001 à EP-003 se réalisent entièrement sous Fedora. 4 epics, 15 stories. |
| 1.2 | 2026-10-29 | Arthur Jean | Revue d'EP-002 : le critère de squash quitte US-005 pour la règle « Fusion et release », puisque la fusion attend EP-003, US-013 et US-014. La definition of done d'EP-002 n'exige plus que les jobs Linux et macOS de `run_tests.yml`, un échec Windows y étant consigné pour EP-004. La moitié « avant » du critère d'échec d'US-007 se prouve en rejouant le contrôle de pin, car la PR bot ne déclenche aucune lane. |

## Problem Statement

**Convention de référence.** Dans tout ce document, `ghostty/` désigne `/home/arthur/dev/ghostty` au tag `v1.4.0` (`83fb95624b0439ee660c2679b43b38c8b63769cd`) ; les numéros de ligne qui suivent ce préfixe sont pris à ce commit. Les chemins sans préfixe désignent Paneflow au commit `ab433d9c`.

Paneflow lie statiquement libghostty-vt au commit Ghostty `b699ea79f4b881421b4b3055abc16a0957d76beb` (`native/libghostty/manifest.toml:3`), une version 1.3.2-dev du 2026-10-06. Ghostty a tagué `v1.4.0` le 2026-10-27. Le pin en est un ancêtre ; 139 commits de l'intervalle touchent `src/terminal`, `include/ghostty`, `src/lib_vt.zig` ou `src/build`.

1. **Paneflow embarque trois défauts mémoire publiés comme avis de sécurité le 2026-10-27.** Les trois correctifs sont absents du pin et présents dans `v1.4.0` (vérifié par `git merge-base --is-ancestor`) :
   - [GHSA-qjfx-r533-6vx3](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-qjfx-r533-6vx3), `65ff54c67` : use-after-free quand le scrollback élague la page du curseur. Le commit précise que la sortie du terminal suffit à le déclencher dès qu'une limite de scrollback est posée. Paneflow pose les deux : en lignes (`crates/paneflow-terminal-ghostty/src/constructor.rs:226-234`) et en octets (`crates/paneflow-terminal-ghostty/src/options.rs:76`, appelé par `crates/paneflow-host/src/runtime.rs:1052`).
   - [GHSA-w9q4-5jvv-mcv4](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-w9q4-5jvv-mcv4), `e257bb45e` : style et hyperlien du curseur invalides après un `scrollClear` qui élague sa page. `ED 22` et `ED 2` sur une invite y mènent (`ghostty/src/terminal/Terminal.zig:3703-3726`).
   - [GHSA-jqr7-69mm-v35h](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-jqr7-69mm-v35h), `0af5c9e55` : nombre de lignes tronqué sur 16 bits quand on rétrécit une page très large. L'avis vise les builds qui contiennent l'API snapshot (`d7bb4b863`), ce qui est le cas du pin.
2. **Le bump est bloqué par la toolchain.** `v1.4.0` exige Zig 0.17.0 (`ghostty/build.zig.zon:6`). Le workflow de bump refuse tout écart avec le manifeste avant de construire (`.github/workflows/libghostty-bump.yml:160-164`). Même après un re-pin de Zig, les trois recettes échoueraient. Vérifié le 2026-10-28 avec Zig 0.17.0 :
   - `zig build --seed 0` répond `error: unrecognized argument: --seed`, alors que `--seed=0` passe. Les trois recettes utilisent la forme en deux arguments (`scripts/build-libghostty-linux.sh:199`, `scripts/build-libghostty-macos.sh:311`, `scripts/build-libghostty-windows.ps1:614`).
   - `zig build` ne connaît plus `--zig-lib-dir`, ni en deux arguments ni avec `=`. La recette Windows en dépend (`build-libghostty-windows.ps1:614`, documenté en `native/libghostty/README.md:145-150`). La variable `ZIG_LIB_DIR` est en revanche honorée : un chemin inexistant échoue avec `unable to open zig lib directory`.
   - Zig 0.16.0 est aussi codé en dur dans `libghostty-bump.yml:312,364`, `libghostty-linux.yml:31`, `libghostty-macos.yml:22`, `release.yml:64` et dans le test `crates/paneflow-terminal-ghostty/src/lib.rs:145`.
3. **Le bump casse la compilation et deux tests.**
   - `ghostty_terminal_vt_write_until_ground` est supprimée (`4d3487ef5`). Elle est remplacée par `ghostty_terminal_vt_write_until(t, data, len, uint32_t boundaries, GhosttyTerminalWriteStop*)` (`ghostty/include/ghostty/vt/terminal.h:3435-3472`). Paneflow l'appelle en `crates/paneflow-terminal-ghostty/src/engine.rs:67` et `:116`.
   - La réponse à `OSC 7501 ; ?` liste désormais les états et les genres (`b7207b2ef`, révision 0.4 du protocole). `crates/paneflow-terminal-ghostty/tests/program_status.rs:133` et `crates/paneflow-host/src/runtime.rs:2609-2614` attendent encore un `?` nu.
   - Seuls deux symboles disparaissent des bindings : la fonction ci-dessus et `GHOSTTY_OSC_COMMAND_CONTEXT_SIGNAL`, renommé et inutilisé par Paneflow. Diff obtenu en régénérant les bindings aux deux commits ; la régénération au pin redonne exactement le `bindings_sha256` commité.
4. **Des changements de comportement atteignent Paneflow sans erreur de compilation.**
   - DECSTR (`CSI ! p`) était ignoré et réinitialise désormais modes, marges, attributs, palette et forme du curseur (`f473f3109`, `ghostty/src/terminal/Terminal.zig:5157-5203`).
   - `ED 2` sur une invite saute les lignes vides (`ff779564c`).
   - Correctifs de graphèmes au retour à la ligne (`b10ba7ea2`, `6dae79db9`).
   - CHA, VPA, HPR et VPR deviennent relatifs à l'origine (`82df0be2c`).
   - CSI 19 t est maintenant pris en charge, et un callback existe pour CSI 8 t.
   - DECRQM répond 4 pour les modes ANSI non pris en charge (`d432b01a3`).
   - Un `\r` dans un collage non bracketé est refusé (`5c9a3d8a9`).

   Les digests golden (`crates/paneflow-terminal-ghostty/tests/snapshot_golden.rs:15-16`) et les baselines de performance peuvent bouger.

**Why now:** les trois avis sont publics depuis le 2026-10-27, donc les correctifs sont lisibles par tous alors que Paneflow livre encore le code vulnérable. L'écart n'a que trois semaines. Upstream annonce encore une rupture d'ABI avant libghostty 1.0, attendue avant Ghostty 1.5 prévu en mars 2027 ([notes 1.4.0](https://ghostty.org/docs/install/release-notes/1-4-0)). Absorber 1.4.0 maintenant laisse au bump suivant une seule rupture à porter. Le passage à Zig 0.17 est inévitable : tout commit upstream postérieur au 2026-10-27 l'exige.

## Overview

Le travail se fait sur une branche `feat/libghostty-v1.4.0`, en quatre epics.

- **EP-001** passe la toolchain à Zig 0.17.0 :
  - re-pin des valeurs Zig dans le manifeste, les workflows et le test d'identité ;
  - adaptation des trois recettes à la nouvelle CLI : `--seed=N`, et `ZIG_LIB_DIR` à la place de `--zig-lib-dir` ;
  - une spike qui prouve la recette d'abord en local sous Linux, puis par un `dry_run` du workflow de bump sur les quatre cibles, avant toute publication.
- **EP-002** publie les archives `v1.4.0` par le workflow de bump, porte le wrapper sur `ghostty_terminal_vt_write_until`, étend la validation ABI, aligne Paneflow sur OSC 7501 révision 0.4, puis aligne lanes, licences, notices, SBOM et documentation.
- **EP-003** prouve côté Paneflow, par un test chacun, les trois correctifs de sécurité et les changements de comportement que Paneflow traverse. Il rejoue les golden, re-baseline les suites terminal Linux, qualifie macOS par la CI et passe une revue visuelle Linux.
- **EP-004**, en dernier comme dans chaque PRD, regroupe tout le travail Windows :
  - l'analyse de l'archive et de la recette Windows produites par le run de bump ;
  - les jobs Windows de la CI ;
  - la passe sur le matériel réel (dual boot).

**Règle d'OS :** EP-001 à EP-003 se réalisent entièrement sous Fedora, sans démarrer Windows. Deux éléments Windows restent hors d'EP-004, parce que le run de bump construit et publie les quatre archives en un seul passage et que le manifeste fige un seul `source_sha` pour toutes les cibles :
- les valeurs Zig Windows du manifeste (US-001), mesurées depuis Fedora sur les archives officielles ;
- les options de `zig build` de la recette PowerShell (US-002), vérifiées par inspection puis par le `dry_run` d'US-003.

Un échec Windows qui bloque le run de bump se corrige depuis Fedora, par des allers-retours en CI ; les autres échecs Windows sont traités en US-013 ou US-014. Seule US-015 demande de démarrer Windows.

**Décisions de conception :**
- **Cible :** le tag `v1.4.0`, pas `main`. Entre les deux, rien ne change sous `include/`, `src/terminal`, `src/lib_vt.zig` ou `src/build`. `main` ajoute en revanche une dépendance `zigimg` non-lazy à la racine de `build.zig.zon` (`b84594f97`), qu'un build `-Demit-lib-vt` irait télécharger.
- **Recette :** Paneflow adapte ses scripts à la CLI de Zig 0.17. Il ne patche ni Zig ni Ghostty. Le README a déjà retiré un patch source pour cette raison (`native/libghostty/README.md:156-161`).
- **Une seule source pour la version de Zig dans le workflow de bump :** les deux étapes `setup-zig` lisent `zig_version` depuis le job `resolve`, qui lit déjà le manifeste (`libghostty-bump.yml:150-153`). Le job Windows lit déjà le manifeste.
- **Commit atomique :** de US-001 à US-006, aucun état intermédiaire ne passe `build.rs` ni la compilation. Le manifeste à Zig 0.17.0 contredit le `build-info.txt` des archives à Zig 0.16.0 (`crates/paneflow-libghostty-sys/src/build_support/manifest.rs:378-382`), puis les bindings v1.4.0 retirent un symbole que `engine.rs` appelle. Ces stories sont donc squashées à la fusion en un seul commit, `chore(libghostty): re-pin libghostty-vt to Ghostty v1.4.0 on Zig 0.17.0`. C'est le précédent du bump GPUI (`tasks/prd-gpui-bump-37faef81.md`, EP-001). Les stories suivantes restent des commits séparés qui compilent.
- **Portage minimal :** `vt_write_until` est appelée avec la seule frontière `GHOSTTY_TERMINAL_BOUNDARY_GROUND`. Les codes de retour gardent leur sens (`SUCCESS` : sol atteint ; `NO_VALUE` : tout consommé), y compris la sonde à longueur nulle d'`at_ground` (`ghostty/include/ghostty/vt/terminal.h:3435-3466`).

**Ce que ce PRD ne fait pas :** il n'adopte aucune nouvelle API de `v1.4.0`, en particulier l'itérateur d'images du render state, `DELTA_PROMPT`, `animation_tick` et le callback CSI 8 t (voir Non-Goals). Si un risque bloquant se confirme, le retour arrière consiste à revenir sur le commit atomique. La pré-release `libghostty-vt-b699ea79…` reste publiée, donc `scripts/fetch-libghostty.sh` retrouve les anciennes archives.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Avis de sécurité libghostty-vt corrigés dans le binaire livré | 3 sur 3 (GHSA-qjfx, GHSA-w9q4, GHSA-jqr7) dans la première release qui suit la fusion | 0 avis publié non corrigé plus de 30 jours après sa publication |
| Écart du pin au dernier tag Ghostty | 0 commit (pin `v1.4.0`) | Pin sur le dernier tag ou la dernière révision libghostty publiée, au plus 4 semaines de retard |
| Correctifs de sécurité couverts par un test Paneflow | 3 tests, un par avis, qui passent sous Linux, puis sous Windows en CI | Chaque correctif de sécurité traversé par un futur bump a son test |
| Recettes reproductibles sous Zig 0.17.0 | Archives Linux x86_64, Linux aarch64 et macOS identiques au bit près sur deux builds | Inchangé à chaque re-pin |
| Régressions terminal attribuées au bump | 0 plantage, 0 régression de rendu reproductible sur les parcours d'EP-003 et d'EP-004 | 0 rapport utilisateur attribué au bump |

## Target Users

### Développeur qui fait tourner des agents CLI dans Paneflow
- **Role:** utilisateur de Paneflow sous Linux, macOS ou Windows, avec plusieurs agents (Claude Code, Codex, OpenCode) et des commandes longues (cargo, terraform, logs de déploiement) côte à côte.
- **Behaviors:**
  - de gros volumes de sortie dans des panes à scrollback borné ;
  - des panes redimensionnés jusqu'à quelques lignes ;
  - des TUI qui effacent l'écran ou envoient des resets ;
  - des sessions gardées par le host d'une mise à jour à l'autre.
- **Pain points:**
  - un host qui peut corrompre sa mémoire ou planter sur une sortie banale, dès que le scrollback élague la page du curseur. Le host garde toutes les sessions : un plantage les perd toutes ;
  - des réponses de terminal moins complètes que l'upstream (DECSTR ignoré, CSI 19 t sans réponse).
- **Current workaround:** aucun. Le défaut se déclenche sans action de l'utilisateur.
- **Success looks like:** les scénarios des trois avis ne font ni planter ni corrompre un terminal, et les TUI qui utilisent DECSTR retrouvent un état propre.

### Mainteneur de Paneflow
- **Role:** Arthur, seul contributeur, et les agents qui implémentent les stories.
- **Behaviors:**
  - re-pin par le workflow de bump ;
  - relecture du diff d'en-têtes ;
  - macOS qualifié par la CI ;
  - Windows vérifié sur le matériel réel (dual boot).
- **Pain points:**
  - un bump de Zig que le workflow refuse sans dire quoi changer ;
  - une version de Zig dupliquée dans neuf endroits ;
  - un README qui décrit une procédure inapplicable (« Re-pin Zig by hand first », `native/libghostty/README.md:238-241`) et un bump hebdomadaire supprimé depuis `ae27f5ed` (`:219`) ;
  - un `header_sha256` qui ne couvre que `vt.h`, identique aux deux commits, et ne voit donc pas les sous-en-têtes.
- **Current workaround:** relire les scripts et les workflows à la main, puis corriger au fil des échecs CI.
- **Success looks like:** un bump de Zig et de Ghostty planifié, un commit atomique, une documentation qui décrit la procédure réelle, et des tests qui prouvent les correctifs traversés.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Autres embarqueurs de libghostty-vt** : le crate [`libghostty-vt`](https://docs.rs/libghostty-vt) et libghostty-rs ([awesome-libghostty](https://github.com/Uzaaft/awesome-libghostty)), [mnml-libghostty-vt](https://docs.rs/crate/mnml-libghostty-vt), [DeBlasis.GhosttyVt](https://www.nuget.org/packages/DeBlasis.GhosttyVt). Tous figent un commit ou suivent la version de l'app, faute de version libghostty taguée. Aucun n'avait migré vers 1.4.0 ou Zig 0.17 au 2026-10-28 (recherche web, non exhaustive). Paneflow ajoute un contrôle d'intégrité plus strict : hash d'archive, attestation et validation ABI à l'exécution.
- **Upstream** ([notes 1.4.0](https://ghostty.org/docs/install/release-notes/1-4-0)) :
  - libghostty « n'est pas directement partie de la release 1.4 », et son API « a rompu la compatibilité plusieurs fois pendant ce cycle » ;
  - une dernière passe sur l'API C, avec une dernière rupture d'ABI, précède libghostty 1.0 et son propre cycle de release, attendus avant Ghostty 1.5.
- **Market gap:** sans objet pour un bump. L'enjeu est de livrer les correctifs de sécurité et de garder le bump suivant à une seule rupture.

### Best Practices Applied
- Bindings régénérés par `scripts/generate-libghostty-bindings.sh` (bindgen 0.72.1, allowlists `^ghostty_.*`, `^Ghostty.*`), jamais édités à la main. La régénération locale au pin redonne `93a2e6832c79…`, et donne `1a4e44af5d9d647b12ec1cc5f680b0955560c820ccd34b7ea406a7f670a0f85a` pour `v1.4.0`.
- Une struct dimensionnée reçoit `size = sizeof`. Sinon `vt_write_until` renvoie `GHOSTTY_INVALID_VALUE` (`ghostty/src/terminal/c/terminal.zig:1145-1154`).
- Chaque discriminant lu est ancré (`crates/paneflow-terminal-ghostty/src/abi.rs:293-330`), et chaque struct lue a un contrôle de layout (`abi_layout.rs:7-19`).
- Un test Paneflow par correctif upstream, nommé `upstream_<sha>_<comportement>` (`crates/paneflow-terminal-ghostty/tests/upstream_fixes.rs:121,406,430`).
- Les valeurs de toolchain sont mesurées sur les archives officielles et non recopiées. Le 2026-10-28 :
  - le ZIP Windows de Zig 0.17.0 a le SHA-256 publié dans `index.json` ;
  - son `zig.exe` vaut `f54b14f0…` ;
  - `ImageBase` (`0x140000000`) et `DllCharacteristics` (`0x8160`) n'ont pas changé ;
  - l'archive source a le SHA-256 publié et garde l'arborescence `zig-0.17.0/lib`.
- **Constat de recherche écarté :** la recherche web annonçait des mises à jour de golden pour l'exécution des C0 `0x10-0x1F` et le rejet des C1 encodés en UTF-8. Ces commits (`0f35043c9`, `40a40f848`) précèdent le pin et sont déjà absorbés.

*Sources : notes de version [Ghostty 1.4.0](https://ghostty.org/docs/install/release-notes/1-4-0) et [Zig 0.17.0](https://ziglang.org/download/0.17.0/release-notes.html), [avis de sécurité Ghostty](https://github.com/ghostty-org/ghostty/security/advisories), diff `b699ea79f..v1.4.0` du clone local, essais de la CLI `zig build` 0.17.0 du 2026-10-28.*

## Assumptions & Constraints

### Assumptions (to validate)
- **A1 :** sous Zig 0.17.0, la recette Linux et macOS (`--seed=0 -j1`, compilateur épinglé sur un CPU, strip, `ar -D`) reste reproductible au bit près. Les drapeaux sont acceptés (vérifié), mais `zig build` est désormais scindé en un configurateur et un exécutable « maker » ([notes Zig 0.17.0](https://ziglang.org/download/0.17.0/release-notes.html)). Validation : US-002 en local, puis US-003.
- **A2 :** `ZIG_LIB_DIR`, pointé sur l'arborescence `lib` de l'archive source, a sous Windows le même effet que l'ancien `--zig-lib-dir`, y compris dans le processus configurateur. Base : la variable est honorée localement. Validation : US-003.
- **A3 :** les invariants de l'archive Windows tiennent sous Zig 0.17.0 :
  - imports limités à `ntdll` et `kernel32` (`manifest.toml:61`) ;
  - CRT statique ;
  - bibliothèques d'import retirées ;
  - horodatage COFF normalisé.

  Doute : Zig 0.17 réécrit le linker COFF. Validation : US-003, puis US-013.
- **A4 :** uucode `ea62149739404a73c202b48a33bf6dd2af4bd9b0` embarque toujours l'Unicode Character Database 18.0.0. Base : la comparaison avec `9d555245` ne touche pas `ucd/`. Validation : US-007, en lisant le paquet téléchargé.
- **A5 :** les noms des membres d'archive cités par les notices (par exemple `libghostty-vt-static_zcu.o`) survivent à Zig 0.17. Non vérifié. Validation : US-007.
- **A6 :** les digests golden et les baselines terminal peuvent bouger, à cause de `b10ba7ea2`, `6dae79db9`, `ff779564c` et du parseur ASCII 5,7 fois plus rapide annoncé par les notes 1.4.0. Chaque mouvement doit s'expliquer par un commit upstream. Validation : US-010.
- **A7 :** `-Doptimize=ReleaseFast` reste accepté sous Zig 0.17.0. Vérifié localement : `standardOptimizeOption` l'accepte et refuse une valeur inconnue. L'alias est déprécié au-delà de 0.18, donc `build_mode` ne change pas maintenant.
- **A8 :** `deps.files.ghostty.org`, d'où uucode est désormais téléchargé (`ghostty/build.zig.zon`), est joignable depuis les runners GitHub. Validation : US-003.

### Hard Constraints
- **Cibles :** quatre cibles livrées (Linux x86_64 et aarch64, macOS aarch64, Windows x86_64), sans régression sur aucune (`AGENTS.md`, « Cross-platform compatibility is mandatory »).
- **Un seul moteur :** libghostty est le seul moteur. Ses types restent dans `paneflow-terminal-ghostty`, et l'app ne voit que les miroirs neutres de `src-app/src/terminal/types.rs`.
- **Pas de commentaires** dans le Rust, le shell ou le PowerShell (`AGENTS.md`).
- **Code Windows :** les items `#[cfg(windows)]` se déclarent avant tout `mod tests` ; seul le job `Windows x86_64 libghostty check` les linte.
- **Commandes cargo :** toujours en `--locked`. Aucune dépendance Rust ne change.
- **Pas de patch** de la source Ghostty ou de Zig dans la recette.
- **Actions externes avec accord d'Arthur :** lancer le workflow de bump (même en `dry_run`), publier une pré-release, fermer la PR bot, supprimer une branche ou une pré-release.
- **Pré-releases :** les anciennes ne sont jamais supprimées. `macos_qualification_package.yml:22-24,134` récupère encore celle de `0c2a290d`.
- **Baselines :** une baseline refuse un arbre sale (`scripts/perf-gates.sh:61`). On commite d'abord, on enregistre ensuite, dans un commit séparé.
- **OS de travail :** EP-001 à EP-003 se font sous Fedora. Tout le travail Windows est dans EP-004, et seule US-015 demande de démarrer Windows.
- **Fusion et release :** le commit atomique n'est fusionné sur `main` qu'une fois les stories P0 d'EP-001 à EP-003, US-013 et US-014 en `IN_REVIEW` ou `DONE`, pour que `main` reste vert sur les quatre legs. À la fusion, les commits d'US-001 à US-006 et le commit bot ne forment qu'un seul commit, `chore(libghostty): re-pin libghostty-vt to Ghostty v1.4.0 on Zig 0.17.0`, qui compile seul ; les stories suivantes restent des commits séparés. Aucun tag de release avant la certification d'US-015.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo test -p paneflow-libghostty-sys --locked` - intégrité du manifeste, des bindings, des notices, du SBOM et des archives

Pendant la fenêtre atomique (US-001 à US-006), ces gates ne peuvent pas passer avant US-006 : la compilation revient avec US-005, les tests avec US-006. Les stories US-001 à US-005 se valident par leurs propres critères, et les gates s'appliquent à l'état final de la fenêtre.

Gates additionnels :
- Stories qui modifient un workflow (US-001, US-007) : `scripts/check-workflow-action-pins.sh`.
- Stories qui modifient une recette (US-002) : la recette concernée s'exécute (Linux en local ; macOS et Windows par US-003).
- Jobs Windows de la CI : ils tournent à chaque push, mais leur traitement relève d'EP-004. Un échec Windows constaté pendant EP-001 à EP-003 est consigné dans la PR et corrigé en US-013 ou US-014. Seule exception : s'il bloque le run de bump (US-003, US-004), il se corrige tout de suite, depuis Fedora et par la CI. La PR dit alors que la correction a été vérifiée par inspection et par la CI.
- Stories de performance (US-010) : `scripts/perf-gates.sh` et `scripts/bench-terminal.sh`, comparés à `bench/baselines/linux-x86_64/`.
- Stories visuelles (US-012, US-015) : passe visuelle d'Arthur sur un build debug, avec une capture ou un enregistrement court joint à la PR. L'agent livre sans lancer l'app pour la vérifier.
- `cargo deny check advisories licenses sources` : seulement si une dépendance Rust change. Aucune story n'en prévoit.

## Epics & User Stories

### EP-001: Passer la recette libghostty à Zig 0.17.0

Faire accepter Zig 0.17.0 par le manifeste, les workflows et les trois recettes, puis prouver que la recette construit `v1.4.0` sur les quatre cibles avant toute publication. Tout se fait sous Fedora. Les valeurs Zig Windows et la recette PowerShell sont des prérequis du run de bump : elles se vérifient par inspection et par la CI, et leur qualification détaillée relève d'US-013.

**Definition of Done:**
- La branche `feat/libghostty-v1.4.0` fige Zig 0.17.0 partout où la version apparaît.
- Les trois recettes utilisent la CLI de Zig 0.17.0.
- Une construction Linux locale reproductible de `v1.4.0` est consignée.
- Un run `dry_run` du workflow de bump est vert sur les quatre cibles, et son identifiant est consigné.

#### US-001: Re-pin Zig 0.17.0 dans le manifeste, les workflows et le test d'identité
**Description:** As a mainteneur de Paneflow, I want que toutes les valeurs Zig de Paneflow désignent la distribution officielle 0.17.0 so that le contrôle préalable du workflow de bump accepte `v1.4.0` et que la recette Windows vérifie les bons binaires.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `native/libghostty/manifest.toml`, when on le réécrit avec `scripts/repin-libghostty-manifest.sh --set ...`, then il contient :
  - `zig_version = "0.17.0"` ;
  - `windows_zig_archive_url = "https://ziglang.org/download/0.17.0/zig-x86_64-windows-0.17.0.zip"` ;
  - `windows_zig_archive_sha256 = "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428"` ;
  - `windows_zig_executable_sha256 = "f54b14f02a4d7d8ac62a94a0663486eb166bba3745d85b466238c1b93e723d46"` ;
  - `windows_zig_source_archive_url = "https://ziglang.org/download/0.17.0/zig-0.17.0.tar.xz"` ;
  - `windows_zig_source_archive_sha256 = "b6c7f1728f043700d6529bac980800792f824256a9d2f1839b3d62beed0b8abd"` ;
  - `windows_zig_image_base` et `windows_zig_dll_characteristics` inchangés (`0x0000000140000000`, `0x8160`).
- [ ] Given ces valeurs, when la PR est ouverte, then elle cite la commande qui a mesuré chacune d'elles sous Fedora sur l'archive officielle téléchargée (`sha256sum`, `objdump -p`), et non une recopie de ce PRD.
- [ ] Given le manifeste, when on lit `macos_canonical_zig_path`, then il vaut `/tmp/paneflow-libghostty-zig-0.17.0`.
- [ ] Given le manifeste, when on lit les licences, then « Zig compiler runtime » a la version `0.17.0` et « LLVM libc++ headers » la version `zig-0.17.0-bundled`.
- [ ] Given `libghostty-bump.yml`, when on lit les deux étapes `mlugg/setup-zig` (`:310-312`, `:362-364`), then leur `version` vient d'une sortie `zig_version` du job `resolve`, lue dans le manifeste, et aucune version littérale n'y subsiste.
- [ ] Given `libghostty-linux.yml:31`, `libghostty-macos.yml:22` et `release.yml:64`, when on les lit, then `ZIG_VERSION` vaut `0.17.0`.
- [ ] Given le dépôt, when on exécute `git grep -n '0\.16\.0' -- .github crates/paneflow-terminal-ghostty/src`, then la commande ne renvoie rien.
- [ ] Given `crates/paneflow-terminal-ghostty/src/lib.rs:145`, when on lit l'assertion, then elle attend `"0.17.0"`.
- [ ] Échec : given ce seul état de la branche, avec les archives `b699ea79` construites sous Zig 0.16.0, when on exécute `cargo build -p paneflow-libghostty-sys --locked`, then `build.rs` refuse en nommant `zig_version`. C'est l'état rouge attendu de la fenêtre atomique ; la PR le dit, et rien n'est poussé sur `main`.

#### US-002: Adapter les trois recettes à la CLI de Zig 0.17.0
**Description:** As a mainteneur de Paneflow, I want que les recettes Linux, macOS et Windows passent leurs options sous une forme que `zig build` 0.17.0 accepte so that les quatre builds du workflow de bump démarrent au lieu d'échouer sur un argument inconnu.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `scripts/build-libghostty-linux.sh:199` et `scripts/build-libghostty-macos.sh:311`, when on lit l'appel à `zig build`, then la graine passe en un seul argument, `--seed="$BUILD_SEED"`.
- [ ] Given `scripts/build-libghostty-windows.ps1:614`, when on lit l'appel à `zig build`, then la graine passe en `"--seed=$BuildSeed"` et `--zig-lib-dir` a disparu.
- [ ] Given la recette Windows, when on la lit (inspection sous Fedora), then `ZIG_LIB_DIR` désigne l'arborescence `lib` vérifiée de l'archive source (`Initialize-ZigSourceLib`, `:784`) pour le seul appel à `zig build`, puis la variable reprend sa valeur précédente, même si le build échoue. Son exécution est prouvée par US-003 et qualifiée en US-013.
- [ ] Given les trois recettes, when on exécute `grep -nE -- '--seed [^=]|--seed "|--zig-lib-dir' scripts/build-libghostty-*`, then la commande ne renvoie rien.
- [ ] Given `native/libghostty/README.md:145-150`, when on le relit, then la commande Windows documentée utilise `--seed=0` et `ZIG_LIB_DIR`, et le texte explique pourquoi l'archive source fournit toujours la bibliothèque Zig.
- [ ] Given Zig 0.17.0 sous Fedora, un checkout propre de Ghostty à `83fb95624b0439ee660c2679b43b38c8b63769cd` et une copie jetable de Paneflow hors du dépôt (`git worktree add` dans le scratch, avec `source_sha` repinné dans cette copie seulement), when on exécute `scripts/build-libghostty-linux.sh --target x86_64-unknown-linux-gnu --verify-reproducible --allow-hash-drift`, then le script termine et les deux builds produisent des archives identiques. La sortie et le SHA-256 de l'archive sont joints à la PR.
- [ ] Échec : given une construction locale non reproductible (deux SHA-256 différents), when la story se termine, then elle passe `BLOCKED` avec les deux hashes et le diff des membres d'archive (`ar t`, `cmp`). La recette n'est pas assouplie.

#### US-003: Validate assumption: la recette Zig 0.17.0 construit v1.4.0 sur les quatre cibles
**Description:** As a mainteneur de Paneflow, I want un run `dry_run` du workflow de bump qui construit `v1.4.0` sur les quatre cibles depuis la branche so that A1, A2, A3 et A8 sont prouvées avant toute publication.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Avant tout lancement, l'agent obtient l'accord explicite d'Arthur : le run consomme des minutes CI sur macOS et Windows.
- [ ] Given la branche poussée, when on lance `gh workflow run libghostty-bump.yml --ref feat/libghostty-v1.4.0 -f source_sha=83fb95624b0439ee660c2679b43b38c8b63769cd -f dry_run=true`, then le job `resolve` affiche `Zig 0.17.0 still applies`.
- [ ] Given ce run, when le job des bindings se termine, then le `bindings_sha256` régénéré vaut `1a4e44af5d9d647b12ec1cc5f680b0955560c820ccd34b7ea406a7f670a0f85a`. Un écart est expliqué dans la PR avant la suite.
- [ ] Given ce run, when les quatre builds se terminent, then les builds Linux et macOS passent `--verify-reproducible`, et le build Windows réussit avec les contrôles intégrés du workflow. L'analyse détaillée de l'archive Windows relève d'US-013.
- [ ] Given ce run, when il se termine, then aucune pré-release n'existe pour `83fb9562`, et l'identifiant du run est consigné dans la PR.
- [ ] Échec : given un build de cible qui échoue (déterminisme perdu, `ZIG_LIB_DIR` ignoré par le configurateur, import inattendu, `deps.files.ghostty.org` injoignable), when le run se termine, then la story passe `BLOCKED` avec l'étape et l'extrait de log en cause. Aucun patch source ni assouplissement de contrôle ; la décision passe par l'Open Question correspondante. Un échec du leg Windows se diagnostique et se corrige depuis Fedora, par la CI, sans démarrer Windows.

---

### EP-002: Re-pin libghostty-vt sur Ghostty v1.4.0 et porter le wrapper

Publier les archives `v1.4.0`, porter le wrapper sur la nouvelle API d'écriture, étendre la validation ABI, aligner Paneflow sur OSC 7501 révision 0.4, et mettre lanes, licences, notices, SBOM et documentation au même commit.

**Definition of Done:**
- `native/libghostty/manifest.toml` fige `83fb95624b0439ee660c2679b43b38c8b63769cd`.
- La pré-release `libghostty-vt-83fb9562…` porte quatre archives attestées.
- Le commit atomique compile et passe les quatre gates sous Linux.
- Les lanes `libghostty-linux.yml` et `libghostty-macos.yml`, et les jobs Linux et macOS de `run_tests.yml`, sont verts sur la PR. Un job Windows en échec y est consigné et relève d'US-013 ou US-014.
- `git grep b699ea79` ne renvoie plus que de l'historique.

#### US-004: Publier les archives v1.4.0 par le workflow de bump
**Description:** As a mainteneur de Paneflow, I want reconstruire, attester et publier les quatre archives `v1.4.0` par le workflow existant so that la provenance soit établie dans le run de bump, comme pour chaque pin précédent.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Avant tout lancement, l'agent obtient l'accord explicite d'Arthur : le run publie une pré-release et ouvre une PR bot.
- [ ] Given le run US-003 vert, when on relance le workflow avec le même `source_sha` sans `dry_run`, then la pré-release `libghostty-vt-83fb95624b0439ee660c2679b43b38c8b63769cd` porte quatre assets nommés `<rust-target>-<archive>` (`libghostty-bump.yml:582-600`).
- [ ] Given ce même run, when il se termine, then la branche `bot/libghostty-83fb9562` existe et la PR bot est ouverte.
- [ ] Given la branche bot, qui contient la branche de feature plus un commit bot, when on avance `feat/libghostty-v1.4.0` dessus, then l'avance est un fast-forward.
- [ ] Given le manifeste du commit bot, when on le lit, then :
  - `ghostty_app_version` vaut `1.4.0+83fb95624` ;
  - `header_sha256` reste `014813d7dba136ae6faecb515d23e7d7dc675672094174f91e2d84ab2bb55732` (`vt.h` est identique aux deux commits) ;
  - `bindings_sha256` vaut `1a4e44af…` ;
  - les chemins canoniques finissent par `83fb9562` ;
  - `windows_source_date_epoch` égale `git -C ghostty show -s --format=%ct 83fb9562` ;
  - la licence libghostty-vt vaut `0.1.0-dev@83fb95624b0439ee660c2679b43b38c8b63769cd`.
- [ ] Given chaque archive téléchargée par `scripts/fetch-libghostty.sh`, when on exécute `gh attestation verify` sur chacune, then les quatre vérifications passent.
- [ ] Given les pré-releases existantes (`b699ea79`, `0c2a290d`, variante `-recipe-11f8a3ef`), when la story se termine, then aucune n'a été modifiée ni supprimée.
- [ ] Échec : given une branche `bot/libghostty-83fb9562` déjà présente, when le workflow arrive à la publication, then il refuse (`libghostty-bump.yml:613-626`). La branche n'est supprimée qu'avec l'accord d'Arthur, après vérification qu'elle n'appartient pas à un bump en cours.
- [ ] Échec : given une publication partielle (pré-release créée, PR non ouverte), when on la constate, then le nettoyage suit `native/libghostty/README.md:230-231` avec l'accord d'Arthur, et le run est relancé.

#### US-005: Porter le wrapper sur ghostty_terminal_vt_write_until et étendre la validation ABI
**Description:** As a mainteneur de Paneflow, I want que `feed` et `at_ground` utilisent `ghostty_terminal_vt_write_until` avec la frontière `GROUND`, et que la validation ABI couvre la nouvelle struct so that le wrapper compile sur `v1.4.0` sans changer le comportement de l'effacement d'historique différé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-terminal-ghostty/src/engine.rs:61-82` (`feed`) et `:112-127` (`at_ground`), when le code est compilé, then les deux appellent `sys::ghostty_terminal_vt_write_until` avec `GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_GROUND as u32` et un `GhosttyTerminalWriteStop` dont `size` vaut `size_of::<GhosttyTerminalWriteStop>()`, et lisent `consumed`.
- [ ] Given `GHOSTTY_SUCCESS` et `GHOSTTY_NO_VALUE`, when `feed` et `at_ground` les reçoivent, then ils produisent les mêmes effets qu'avant le bump, et les noms d'opération passés à `check` deviennent `terminal_vt_write_until`.
- [ ] Given le workspace, when on exécute `git grep -n vt_write_until_ground -- crates src-app native/libghostty/windows-smoke.c`, then la commande ne renvoie rien.
- [ ] Given `crates/paneflow-terminal-ghostty/src/abi_discriminants.rs`, when on lit `READ_DISCRIMINANTS`, then il contient `GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_GROUND`, et `every_discriminant_the_crate_reads_is_validated` (`abi.rs:293`) passe.
- [ ] Given `crates/paneflow-terminal-ghostty/src/abi_layout.rs`, when la validation tourne, then elle vérifie la taille de `GhosttyTerminalWriteStop`, et l'offset et la taille de ses sept champs (`size`, `consumed`, `reached`, `kind`, `introducer`, `end`, `byte`), contre `ghostty_type_json()`.
- [ ] Given un terminal à qui on envoie la moitié d'une séquence `OSC 0` (sans terminateur), when on appelle `at_ground`, then il renvoie faux. Une fois le terminateur `ST` envoyé, il renvoie vrai. Le test vit dans les tests du wrapper.
- [ ] Given un effacement d'historique différé (`CSI 3 J`) au milieu d'un lot qui finit dans une séquence, when le lot est consommé, then l'historique n'est effacé qu'après le retour au sol. Les tests existants du chemin différé restent verts.
- [ ] Échec : given un JSON de layout altéré qui décale `GhosttyTerminalWriteStop.consumed`, when la validation tourne, then elle renvoie `AbiMismatch` en nommant la struct et le champ. Testé sur une copie altérée, selon le motif existant.

#### US-006: Aligner Paneflow sur le protocole OSC 7501 révision 0.4
**Description:** As a utilisateur qui lance des programmes compatibles OSC 7501, I want que le host réponde à la requête de support avec la liste des états et des genres de la révision 0.4 so that un programme sache exactement ce que Paneflow accepte, et que les tests de Paneflow décrivent le moteur réellement embarqué.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un terminal hôte avec le callback de statut de programme, when il reçoit `ESC ] 7501 ; ? ESC \`, then la seule écriture PTY est `ESC ] 7501 ; ?:states=idle,working,done,blocked,error:kinds=permission,question,auth ESC \`. Les assertions sont mises à jour en `crates/paneflow-terminal-ghostty/tests/program_status.rs:133` et `crates/paneflow-host/src/runtime.rs:2609-2614`.
- [ ] Given une session hôte, when le programme envoie la requête, then la réponse atteint le PTY exactement une fois (`a_program_status_query_reaches_the_pty_exactly_once`).
- [ ] Given un enregistrement `working` ou `blocked` sans pourcentage, when le host le tient, then `progress` vaut `None`, ce que la révision 0.4 appelle « travail indéterminé ».
- [ ] Given l'exemple de la révision 0.4 (une racine avec `app` = `deploy`, puis un enregistrement `us-east` sans `app`), when le host les applique, then `us-east` porte `app` = `deploy`. Comportement existant (`crates/paneflow-host/src/program_status.rs:129-144`), désormais couvert par un test qui cite l'exemple.
- [ ] Échec : given un terminal desktop, sans callback de statut de programme, when il reçoit la requête, then rien n'est écrit au PTY (assertion existante, `program_status.rs:128-129`).

#### US-007: Aligner lanes, licences, notices, SBOM et documentation sur v1.4.0
**Description:** As a mainteneur de Paneflow, I want que les lanes, l'inventaire de licences, les notices, le SBOM, les runbooks et le CHANGELOG désignent `v1.4.0` et Zig 0.17.0 so that les lanes de release reconstruisent le bon commit, que l'inventaire distribué soit exact, et que le prochain bump de Zig suive une procédure écrite et applicable.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given `.github/workflows/libghostty-linux.yml:30`, `libghostty-macos.yml:21` et `release.yml:63`, when on les lit, then `GHOSTTY_SHA` vaut `83fb95624b0439ee660c2679b43b38c8b63769cd`.
- [ ] Given la licence uucode du manifeste, when on la lit, then sa version vaut `0.2.0@ea62149739404a73c202b48a33bf6dd2af4bd9b0`.
- [ ] Given le paquet uucode réellement téléchargé par le build, when on lit sa version d'UCD, then la valeur écrite dans le manifeste, les notices et le SBOM est celle-là. La PR cite la valeur et le fichier lu.
- [ ] Given les nouvelles archives, when on liste leurs membres (`llvm-ar t`), then chaque nom de membre cité dans `native/libghostty/THIRD_PARTY_NOTICES.md` existe, ou la notice est corrigée.
- [ ] Given `THIRD_PARTY_NOTICES.md` et `native/libghostty/sbom.cdx.json`, when on les relit, then les empreintes d'archives, la version de libghostty-vt, Zig 0.17.0, les en-têtes libc++ et uucode correspondent au manifeste.
- [ ] Given ces deux fichiers, when on met à jour `notice_sha256` et `sbom_sha256` par `scripts/repin-libghostty-manifest.sh`, then `paneflow-libghostty-sys/build.rs` accepte le résultat.
- [ ] Given le dépôt, when on exécute `git grep b699ea79`, then seuls les fichiers historiques répondent : `CHANGELOG.md` antérieur, `bench/results/`, `bench/baselines/`, `docs/release/qualification/` et `tasks/`.
- [ ] Given `native/libghostty/README.md`, when on le relit, then :
  - les mentions du pin désignent `v1.4.0` ;
  - `:219` dit que le bump se lance à la demande (cron retiré par `ae27f5ed`) ;
  - `:238-241` décrit la procédure réelle d'un bump de Zig : re-pin Zig sur une branche, puis `workflow_dispatch` depuis cette branche ;
  - le message d'erreur de `libghostty-bump.yml:162` renvoie à cette section.
- [ ] Given `docs/release/macos-libghostty.md:228-251` et `docs/release/libghostty-linux.md:37`, when on les relit, then ils citent `83fb9562` et Zig 0.17.0.
- [ ] Given `CHANGELOG.md`, section `[Unreleased]`, when on la lit, then elle contient :
  - sous « Security », les trois avis avec leur lien ;
  - sous « Changed », DECSTR, CSI 19 t, DECRQM pour les modes ANSI non pris en charge et la réponse OSC 7501 révision 0.4 ;
  - sous « Upgrade notes », la puce existante « Sessions that outlive the update end as `Incompatible` » (modèle : section 0.17.7).
- [ ] Échec : given une notice ou un SBOM modifié sans hash à jour, when `cargo build` s'exécute, then `build.rs` refuse en nommant le fichier (test existant gardé vert).
- [ ] Échec : given le contrôle de pin de la lane Linux (`libghostty-linux.yml:121-135`) rejoué localement avec l'ancien `GHOSTTY_SHA` contre le manifeste du commit bot, when il s'exécute, then il échoue en nommant `source_sha`. La PR bot ne déclenche aucune lane, d'où ce rejeu. Sur la PR de la feature, après la mise à jour, la lane passe ce contrôle.

---

### EP-003: Prouver les correctifs traversés et requalifier Linux et macOS

Prouver côté Paneflow les trois correctifs de sécurité et les changements de comportement atteignables, expliquer chaque mouvement de golden et de baseline, qualifier macOS par la CI et passer une revue visuelle Linux.

**Definition of Done:**
- Trois tests de sécurité et les tests de comportement passent sous Linux. Leur exécution sous Windows relève d'US-014.
- Chaque digest golden et chaque baseline qui bouge a une cause écrite.
- Les jobs macOS de `run_tests.yml` et `libghostty-macos.yml` sont verts.
- La passe visuelle Linux est approuvée par Arthur.

#### US-008: Tester les trois correctifs de sécurité traversés
**Description:** As a mainteneur de Paneflow, I want un test Paneflow par avis de sécurité, piloté par l'API du wrapper so that un futur re-pin qui réintroduirait l'un de ces défauts échoue dans notre CI et pas chez un utilisateur.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-terminal-ghostty/tests/upstream_fixes.rs`, when on ajoute les tests, then ils suivent la convention `upstream_<sha>_<comportement>` et le gating existant du fichier (`:1-7`).
- [ ] GHSA-qjfx (`65ff54c67`, oracle `ghostty/src/terminal/Terminal.zig`, test « linefeed on single row screen prunes cursor page ») : given un terminal d'une ligne avec une limite de scrollback en lignes posée par le wrapper, when on envoie 1 000 lignes avec un style et un hyperlien OSC 8 actifs, then rien ne panique, le texte de la dernière ligne est celui attendu, et une impression suivante garde le style.
- [ ] GHSA-w9q4 (`e257bb45e`, oracle `Terminal.zig:7964`) : given un curseur avec un style non par défaut, un hyperlien OSC 8 actif et une petite limite de scrollback, when le programme envoie `CSI 22 J` assez de fois pour élaguer la page du curseur, then une impression suivante garde le style et l'hyperlien sans panique.
- [ ] GHSA-w9q4, chemin de l'invite : given un shell qui a émis `OSC 133 ; A`, when le programme envoie `CSI 2 J` (qui devient un `scroll_complete` sur une invite, `Terminal.zig:3726`), then le même invariant tient.
- [ ] GHSA-jqr7 (`0af5c9e55`, oracle `ghostty/src/terminal/snapshot/screen.zig`) : given la largeur maximale que le wrapper accepte, when on redimensionne vers une largeur beaucoup plus étroite, then aucun nombre de lignes n'est tronqué : le nombre de lignes et le texte publiés sont cohérents.
- [ ] GHSA-jqr7, round trip : given la même largeur, when un snapshot est encodé, puis décodé et rétréci, then le même invariant tient.
- [ ] GHSA-jqr7, limites : given des limites du wrapper (`crates/paneflow-terminal-ghostty/src/limits.rs`) qui rendent la largeur de 65 535 colonnes inatteignable, when le test s'exécute, then il affirme le refus à la frontière, et la PR dit pourquoi le scénario upstream n'est pas atteignable.
- [ ] Given ces trois tests exécutés contre les archives `b699ea79` (en local, avant le squash), when on consigne leur résultat, then la PR indique lesquels échouent ou plantent sur l'ancien moteur, ce qui prouve qu'ils couvrent le défaut.
- [ ] Échec : given un scénario d'avis qui n'est atteignable par aucune API du wrapper, when on l'écrit, then la story le consigne avec la chaîne d'appels manquante et n'ajoute pas d'API au wrapper pour le seul test.

#### US-009: Tester les changements de comportement VT que Paneflow traverse
**Description:** As a utilisateur de TUI et de shells instrumentés, I want que DECSTR, ED 2 sur une invite, CSI 19 t, CSI 8 t et le collage avec `\r` se comportent de façon prouvée dans Paneflow so that le thème, la géométrie des panes et la sécurité du collage restent ceux de Paneflow après le bump.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given le thème Paneflow appliqué par `set_palette` et un `OSC 4` qui change la couleur 1, when le programme envoie `CSI ! p` (DECSTR, `f473f3109`), then `palette_reply` renvoie la couleur du thème, pas la palette par défaut de Ghostty. Le test calque `upstream_bb20f8e45_ris_restores_the_configured_palette` (`upstream_fixes.rs:430-453`).
- [ ] Given des modes DECOM, IRM et DECAWM modifiés et une région de défilement posée, when le programme envoie DECSTR, then ces modes reviennent à leur défaut, la région couvre tout l'écran, et le texte affiché ne change pas.
- [ ] Given un DECSTR, when il s'exécute, then le callback de reset de Paneflow n'est pas appelé (`DisplayTerminal::reset`, `engine.rs:98-110`), et aucun état d'agent n'est effacé.
- [ ] Given un shell instrumenté OSC 133 à l'invite, when le programme envoie `CSI 2 J` (`ff779564c`), then les lignes vides ne partent pas dans le scrollback, et le texte non vide reste retrouvable par la recherche native.
- [ ] Given un terminal de 24 lignes et 80 colonnes, when le programme envoie `CSI 19 t`, then l'écriture PTY est `CSI 9 ; 24 ; 80 t`.
- [ ] Given un programme qui envoie `CSI 8 ; 50 ; 100 t`, when la séquence est traitée, then la taille du terminal ne change pas et rien n'est écrit au PTY. `GHOSTTY_TERMINAL_OPT_RESIZE_WINDOW` reste non posé, ce qui ignore la requête (`ghostty/include/ghostty/vt/terminal.h:2775-2782`).
- [ ] Given `DECRQM` sur le mode ANSI 5 (SRTM, non pris en charge), when le programme l'envoie, then la réponse est `CSI 5 ; 4 $ y` (`d432b01a3`). Le test existant `upstream_9dc0d974e_and_3beb6d717_...` (`upstream_fixes.rs:406`) reste vert sans changement.
- [ ] Given un texte de presse-papiers `"a\rb"` collé hors mode bracketed paste, when il passe par `write_paste_text_from` (`src-app/src/terminal/input.rs:1222-1226`), then `normalize_paste_text` en fait `"a\nb"` avant le moteur, et la confirmation multi-ligne s'applique (`input.rs:1228-1269`). Un test vérifie que `normalize_paste_text` ne laisse aucun `\r` isolé.
- [ ] Échec : given un appel direct à `write_paste` du wrapper avec `"a\rb"`, sans bracketed paste et avec `allow_unsafe = false`, when le moteur le refuse, then le wrapper renvoie `GhosttyError::UnsafePaste` sans panique, et rien n'atteint le PTY.

#### US-010: Rejouer les golden et re-baseliner les suites terminal Linux
**Description:** As a mainteneur de Paneflow, I want des digests golden et des baselines terminal enregistrés sur le commit du bump, avec la cause de chaque écart so that les gates de performance et de rendu comparent les futures modifications au moteur réellement embarqué.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-terminal-ghostty/tests/snapshot_golden.rs:15-16`, when les tests golden s'exécutent sur le commit atomique, then soit ils passent sans changement, soit chaque nouveau digest est accompagné, dans le message de commit, de la séquence du corpus en cause et du commit upstream qui l'explique (par exemple `b10ba7ea2`, `6dae79db9` ou `ff779564c`).
- [ ] Given un arbre propre au commit atomique, when on exécute `scripts/bench-terminal.sh` sous Linux x86_64, then le rapport est archivé dans `bench/results/` avec le SHA du commit et une comparaison au rapport GPUI de `92a5c29d` (`bench/results/gpui-bump-37faef81-2026-10-28.md`).
- [ ] Given chaque métrique qui dépasse sa tolérance, when on la constate, then sa cause est écrite avant tout rafraîchissement (commit upstream, par exemple l'accélération du parseur ASCII, ou changement Paneflow).
- [ ] Given `terminal-alloc.json` et la baseline `persistent`, when un rafraîchissement est justifié, then il passe par `scripts/perf-gates.sh --refresh-alloc-baselines` et par le script de la suite persistante, sur un arbre propre, dans un commit séparé du commit atomique.
- [ ] Given les tests `every_committed_baseline_is_clean_current_and_on_its_platform` (crate `paneflow-app`) et `every_committed_persistent_baseline_is_clean_current_and_on_its_platform` (crate `paneflow-host`), when ils lisent `bench/baselines/`, then ils passent.
- [ ] Given la PR, when le job `Performance gates (Linux x86_64)` s'exécute, then il est vert.
- [ ] Échec : given une régression au-delà de la tolérance sans cause identifiée, when elle est constatée, then la baseline n'est pas rafraîchie et la story passe `BLOCKED` avec la mesure.

#### US-011: Qualifier macOS par la CI
**Description:** As a mainteneur de Paneflow, I want que les jobs macOS de `run_tests.yml` et la lane `libghostty-macos.yml` passent sur la branche, reconstruction reproductible comprise so that le moteur `v1.4.0` construit sous Zig 0.17.0 est qualifié sur la seule cible sans matériel garanti.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given la PR, when `run_tests.yml` s'exécute, then `macOS aarch64 smoke build` et `macOS aarch64 render smoke (visual)` sont verts.
- [ ] Given l'accord d'Arthur, when on lance `libghostty-macos.yml` en `workflow_dispatch` sur la branche, then la reconstruction depuis la source (`:74-76`) produit une archive identique à l'asset publié, et la lane consommatrice (`:178-247`) passe.
- [ ] Given les captures du render smoke, when on les compare à celles du dernier run vert de `main`, then aucune différence de rendu n'est inexpliquée.
- [ ] Échec : given une reconstruction macOS différente de l'asset, when elle est constatée, then la story passe `BLOCKED` avec le diff des membres d'archive. L'asset n'est pas republié sans un nouveau run du workflow de bump.

#### US-012: Passe visuelle Linux sur le nouveau moteur
**Description:** As a utilisateur Linux, I want que les agents, les TUI, le redimensionnement, les images Kitty et la restauration de session se comportent comme avant le bump so that le re-pin ne livre aucune régression visible.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given un build debug sous Wayland, when Arthur lance Claude Code, Neovim, htop et `fzf` dans quatre panes, then le rendu et la saisie sont identiques à ceux de `main` avant le bump, captures à l'appui.
- [ ] Given un pane réduit à une ligne pendant un `cargo build` verbeux, when la sortie défile pendant 60 secondes, then l'application ne plante pas et le pane reste lisible une fois agrandi.
- [ ] Given `kitty +kitten icat` sur une image PNG, when l'image s'affiche puis que le pane défile, then elle reste à sa place.
- [ ] Given un programme qui envoie `CSI ! p` après avoir changé la palette, when la séquence est traitée, then les couleurs reviennent au thème Paneflow.
- [ ] Given un host lancé par la version précédente et un desktop construit sur `v1.4.0`, when le desktop s'attache, then la session affiche `Incompatible` avec « Retry » et « Stop host and restart » (`src-app/src/terminal/host_link.rs:254,355-365`), et « Stop host and restart » démarre le nouveau host.
- [ ] Échec : given une régression visible, when Arthur la constate, then elle est consignée avec sa capture et son scénario, et la story passe `BLOCKED` jusqu'à sa correction ou sa justification par un commit upstream.

---

### EP-004: Windows : archive, CI et matériel réel

Tout le travail Windows du bump est regroupé ici : l'analyse de l'archive et de la recette Windows produites par le run de bump, les jobs Windows de la CI, puis la passe sur le matériel réel. US-013 et US-014 se font depuis Fedora, par la CI. Seule US-015 demande de démarrer Windows. Comme dans chaque PRD, cet epic vient en dernier.

**Definition of Done:**
- L'archive Windows et sa recette sont qualifiées, avec chaque écart d'inventaire expliqué.
- `Windows x86_64 libghostty check`, `Windows x86_64 render smoke (visual)` et `libghostty-windows.yml` sont verts sur la branche.
- La passe matérielle sous Windows 11 est approuvée par Arthur.
- Le rapport de qualification est consigné dans `docs/release/qualification/`.

#### US-013: Qualifier l'archive et la recette Windows depuis la CI
**Description:** As a mainteneur de Paneflow, I want analyser depuis Fedora l'archive Windows publiée et faire passer la lane de qualification Windows so that A2 et A3 sont confirmées, et que chaque écart de l'archive Windows est expliqué avant toute passe matérielle.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-007

**Acceptance Criteria:**
- [ ] Given le journal `--verbose` du build Windows du run US-004, when on le lit, then la bibliothèque Zig utilisée est celle de l'archive source vérifiée et non celle du ZIP, ce qui confirme A2. La ligne du journal est citée dans la PR.
- [ ] Given l'inventaire Windows publié (`build-info.txt`, `symbols.txt`, index des en-têtes), when on le compare à celui du pin, then les écarts de `symbols_sha256` et de `headers_index_sha256` sont expliqués dans la PR. Attendus : `ghostty_terminal_vt_write_until` remplace `vt_write_until_ground`, plus six symboles ajoutés et les effets de `b68ed392b` (highway sans stack protector sous MSVC).
- [ ] Given l'archive Windows, when on liste ses imports et son CRT, then les imports sont limités à `ntdll` et `kernel32` (`manifest.toml`, `system_libraries`), le CRT est statique et les bibliothèques d'import sont retirées, ce qui confirme A3.
- [ ] Given l'accord d'Arthur, when on lance `libghostty-windows.yml` sur la branche, then `qualify-libghostty-windows.ps1` passe sur windows-2025 (`docs/release/windows-libghostty.md:14-44`) : P95 de performance, 200 cycles, stress à 32 panes, imports PE.
- [ ] Échec (inspection) : given une archive source de Zig absente ou corrompue, when la recette Windows prépare `ZIG_LIB_DIR`, then `Initialize-ZigSourceLib` lève une erreur qui nomme le fichier ou le chemin en cause, avant tout `zig build` (`scripts/build-libghostty-windows.ps1:498-530`). La PR dit que ce chemin a été vérifié par inspection.
- [ ] Échec : given un import PE hors de `ntdll` et `kernel32`, when la qualification le détecte, then la story passe `BLOCKED` avec la liste d'imports, et `system_libraries` n'est pas élargi sans l'accord d'Arthur.

#### US-014: Faire passer les jobs Windows de la CI
**Description:** As a mainteneur de Paneflow, I want que la cible Windows compile, passe clippy et exécute tous les tests du bump avec l'archive Zig 0.17.0 so that le code du portage et les tests des correctifs sont prouvés sous Windows avant la passe matérielle, sans démarrer Windows.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006, US-008, US-009

**Acceptance Criteria:**
- [ ] Given la PR, when `run_tests.yml` s'exécute, then `Windows x86_64 libghostty check` (clippy `--target x86_64-pc-windows-msvc -D warnings` et tests) est vert.
- [ ] Given ce job, when il exécute `upstream_fixes.rs` et `program_status.rs`, then les tests d'US-006, d'US-008 et d'US-009 passent sous Windows MSVC : le fichier est gated pour cette cible (`upstream_fixes.rs:1-7`).
- [ ] Given la PR, when `Windows x86_64 render smoke (visual)` s'exécute, then il est vert et ses captures ne montrent aucune différence inexpliquée par rapport au dernier run vert de `main`.
- [ ] Given `native/libghostty/windows-smoke.c`, when le smoke s'exécute, then il passe sans changement de source (il n'appelle aucune API supprimée).
- [ ] Échec : given un échec de clippy sur du code `#[cfg(windows)]`, when on le corrige depuis Fedora, then le nouvel item est déclaré avant tout `mod tests` (`clippy::items_after_test_module`), et la PR dit que la correction a été vérifiée par inspection et par la CI.

#### US-015: Passe matérielle Windows sur le nouveau moteur
**Description:** As a utilisateur Windows 11, I want que les agents, les TUI, le collage, DECSTR, le redimensionnement et la restauration de session se comportent comme avant le bump sur ma machine so that le re-pin est livrable sur la seule cible vérifiée hors de la CI.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013, US-014

**Acceptance Criteria:**
- [ ] Given la branche récupérée sous Windows 11 (dual boot) et `scripts/dev.ps1`, when Arthur ouvre PowerShell, Claude Code et Neovim dans trois panes, then le rendu et la saisie sont identiques à ceux de `main` avant le bump.
- [ ] Given un pane réduit à une ligne pendant une sortie continue de 60 secondes, when la sortie défile, then l'application ne plante pas.
- [ ] Given un texte multi-ligne avec des fins de ligne `\r\n` copié depuis le Bloc-notes, when il est collé dans PowerShell, then la confirmation multi-ligne s'affiche ou le collage bracketé s'applique, sans ligne vide parasite.
- [ ] Given un programme qui envoie `CSI ! p` après un changement de palette, when la séquence est traitée, then les couleurs reviennent au thème Paneflow.
- [ ] Given un host lancé par la version précédente, when le desktop `v1.4.0` s'attache, then le parcours `Incompatible`, puis « Stop host and restart », fonctionne.
- [ ] Given la passe, when elle se termine, then un rapport `docs/release/qualification/windows-<date>.md` consigne la machine, le build, les scénarios et leur verdict, selon le format de `docs/release/qualification/windows-20260922.md`.
- [ ] Échec : given un écart propre à Windows, when Arthur le constate, then il est corrigé dans cette story avec un test lorsque c'est possible, ou consigné en `BLOCKED` avec son scénario. La release attend la correction.

## Functional Requirements

- FR-01: Le manifeste, les workflows et les recettes doivent désigner Zig 0.17.0 et Ghostty `83fb95624b0439ee660c2679b43b38c8b63769cd`.
- FR-02: Le workflow de bump doit lire la version de Zig dans le manifeste ; aucune version de Zig littérale ne doit subsister dans `libghostty-bump.yml`.
- FR-03: Les recettes doivent passer la graine sous la forme `--seed=N`, et la recette Windows doit fournir la bibliothèque Zig de l'archive source par `ZIG_LIB_DIR`.
- FR-04: Le wrapper doit consommer l'entrée PTY par `ghostty_terminal_vt_write_until` avec la frontière `GROUND`, en conservant la sémantique d'`at_ground` et de l'effacement d'historique différé.
- FR-05: La validation ABI à la construction d'un terminal doit couvrir `GHOSTTY_TERMINAL_BOUNDARY_GROUND` et le layout complet de `GhosttyTerminalWriteStop`.
- FR-06: Le host doit répondre à `OSC 7501 ; ?` avec la liste des états et des genres de la révision 0.4, exactement une fois ; un terminal desktop ne doit pas répondre.
- FR-07: Le système ne doit pas poser `GHOSTTY_TERMINAL_OPT_RESIZE_WINDOW` : une requête CSI 8 t ne doit jamais changer la géométrie d'un pane.
- FR-08: Un DECSTR doit ramener la palette au thème Paneflow.
- FR-09: Les notices, le SBOM et l'inventaire de licences doivent décrire les composants réellement liés, versions lues dans les paquets téléchargés.
- FR-10: La recette ne doit appliquer aucun patch à la source de Ghostty ou de Zig.

## Non-Functional Requirements

- **Performance :**
  - chaque métrique de `bench/baselines/linux-x86_64/terminal.json` reste dans sa tolérance commitée, ou son écart est expliqué par un commit nommé avant rafraîchissement ;
  - 0 dépassement non expliqué sur `terminal-alloc` et `persistent` ;
  - sous Windows, la lane de qualification passe ses seuils P95 existants (`docs/release/windows-libghostty.md:14-44`).
- **Sécurité :**
  - 3 avis sur 3 corrigés, chacun couvert par 1 test Paneflow qui échoue ou plante sur l'ancien moteur ;
  - 4 archives sur 4 attestées et vérifiées par `gh attestation verify` ;
  - le SBOM cite le `source_sha` du manifeste.
- **Reproductibilité :** les archives Linux x86_64, Linux aarch64 et macOS aarch64 sont identiques au bit près sur 2 builds sur 2. L'archive Windows reste construite une fois, avec attestation (`native/libghostty/README.md:152-170`).
- **Fiabilité :**
  - 0 panique sur 1 000 itérations de chaque scénario d'US-008 ;
  - 0 plantage pendant les 60 secondes de sortie continue d'US-012 et d'US-015 ;
  - 0 plantage au stress à 32 panes de la lane Windows.
- **Taille :**
  - plafonds inchangés : `paneflow-shim` 512 KiB, `paneflow-ai-hook` environ 375 KB, `paneflow-mcp` 512 KiB ;
  - la taille de chaque archive libghostty est consignée dans la PR ;
  - le binaire release Linux x86_64 ne grossit pas de plus de 5 % par rapport à `ab433d9c`, sinon l'écart est expliqué.
- **Accessibilité :** sans objet, aucun changement d'interface.
- **Montée en charge :** le stress à 32 panes de la lane Windows et le stress PTY de la lane Linux passent sans modification de leurs seuils.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Graine en deux arguments | Une recette passe `--seed 0` à Zig 0.17.0 | Le build échoue avant de compiler ; US-002 l'empêche par la forme `--seed=0` et un `grep` de contrôle | `error: unrecognized argument: --seed` (journal CI) |
| 2 | Bibliothèque Zig introuvable | Archive source de Zig absente ou corrompue | La recette Windows s'arrête avant `zig build`, en nommant le chemin (US-013) | Message de la recette nommant le chemin (journal CI) |
| 3 | Panne de `deps.files.ghostty.org` | Le build télécharge uucode pendant le bump | Le run échoue sans publier ; la story passe `BLOCKED` ; aucun miroir non vérifié n'est substitué | Aucun (journal CI) |
| 4 | Branche bot déjà présente | Un bump précédent n'a pas été soldé | Le workflow refuse (`libghostty-bump.yml:613-626`) ; suppression seulement avec l'accord d'Arthur | `bot/libghostty-83fb9562 already exists...` (journal CI) |
| 5 | Publication partielle | La pré-release est créée, la PR ne s'ouvre pas | Nettoyage selon `native/libghostty/README.md:230-231`, puis relance | Aucun |
| 6 | Host d'une version antérieure | Mise à jour hors de l'app, ou « Keep sessions running » | La session finit `Incompatible` ; « Stop host and restart » démarre le nouveau host | Libellés existants `Retry` et `Stop host and restart` |
| 7 | Terminal d'une ligne avec scrollback borné | Pane réduit au minimum pendant une sortie continue (GHSA-qjfx) | Aucun plantage ni corruption ; test d'US-008 | Aucun |
| 8 | Page très large rétrécie | Redimensionnement depuis la largeur maximale (GHSA-jqr7) | Nombre de lignes cohérent, pas de troncature ; test d'US-008 | Aucun |
| 9 | DECSTR après un changement de palette | Un TUI envoie `CSI ! p` | La palette revient au thème Paneflow, pas aux couleurs de Ghostty | Aucun |
| 10 | Demande de redimensionnement de fenêtre | Un programme envoie `CSI 8 ; r ; c t` | Ignorée : la géométrie du pane ne change pas | Aucun |
| 11 | Collage avec `\r` isolé | Presse-papiers contenant `a\rb` hors bracketed paste | Normalisé en `a\nb`, puis confirmation multi-ligne ; un appel direct non normalisé est refusé sans panique | Invite existante de collage multi-ligne |
| 12 | Retour arrière | Un risque bloquant après fusion | `git revert` du commit atomique ; la pré-release `b699ea79` reste téléchargeable | Puce « Upgrade notes » inversée dans le CHANGELOG de la release corrective |
| 13 | Snapshot `v1.4.0` lu par un moteur antérieur | Retour arrière avec des checkpoints de host | Sans effet : la limite de stockage Kitty vaut 0 pour tout terminal non desktop (`constructor.rs:189-224`), donc aucun enregistrement d'image n'est encodé | Aucun |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Zig 0.17.0 rend la recette Linux ou macOS non reproductible (configurateur séparé, nouveau linker ELF « mostly-reproducible ») | Medium | High | Construction locale reproductible en US-002, puis `dry_run` en US-003 avant toute publication ; décision par l'Open Question 2 si A1 tombe |
| 2 | Le nouveau linker COFF de Zig 0.17 change les imports, le CRT ou la normalisation de l'archive Windows | Medium | High | Contrôles intégrés du run en US-003, puis analyse de l'archive et lane de qualification en US-013 ; aucun élargissement de `system_libraries` sans accord |
| 3 | `ZIG_LIB_DIR` n'atteint pas le configurateur de `zig build` sous Windows | Low | High | Validation par le `dry_run` d'US-003 ; repli : lancer `zig build` depuis une copie canonique de Zig qui embarque la bibliothèque de l'archive source, comme la recette macOS |
| 4 | Digests golden ou baselines déplacés par des changements upstream légitimes, masquant une vraie régression | Medium | Medium | Une cause écrite par mouvement avant tout rafraîchissement (US-010) ; `BLOCKED` sans cause |
| 5 | Une rupture d'ABI upstream supplémentaire avant libghostty 1.0 rend une partie du portage caduque | High | Low | Portage minimal, aucune adoption de nouvelle API dans ce PRD |
| 6 | Régression VT de 1.4.0 découverte après la release (1.3.1 a corrigé des régressions de 1.3.0 un mois plus tard) | Medium | Medium | Passes visuelles Linux et Windows ; le retour arrière se fait par un seul revert ; surveiller les issues upstream jusqu'à la release |
| 7 | Une passe Windows séparée de celle du bump GPUI double le coût des redémarrages | Medium | Low | Open Question 1 : une seule session matérielle couvrant EP-003 du PRD GPUI et US-015 de ce PRD |

## Non-Goals

- **Itérateur d'images du render state pour Kitty** (`ghostty_render_state_image_iterator_*`, `GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR`, `IMAGE_PLACEMENTS_GENERATION`) : il dessinerait les placements virtuels et les images en overscan, que `src-app/src/terminal/kitty.rs:140-193` ignore aujourd'hui. Il demande de 400 à 600 LOC et une passe visuelle sur trois plateformes ; c'est une fonctionnalité, pas un bump. PRD de suivi recommandé, après la dernière rupture d'ABI annoncée par upstream.
- **`GHOSTTY_SCROLL_VIEWPORT_DELTA_PROMPT`** : il remplacerait `MarkRing` et `Osc133Scanner` (`src-app/src/terminal/marks.rs`), dont les positions dérivent une fois le scrollback plein. Mais `prompt_start_seq` alimente aussi la récolte des sessions au retour du shell, donc la suppression n'est que partielle. PRD de suivi recommandé.
- **`ghostty_kitty_graphics_animation_tick`** : les animations Kitty sont une nouvelle fonctionnalité.
- **Callback CSI 8 t** (`GHOSTTY_TERMINAL_OPT_RESIZE_WINDOW`) : un multiplexeur garde la main sur la géométrie des panes ; la requête reste ignorée.
- **Pin sur `main` upstream** : rien pour lib-vt, et une dépendance `zigimg` à télécharger en plus.
- **Renommage de `build_mode` en `fast`** : `ReleaseFast` reste accepté jusqu'à Zig 0.18.
- **Reproductibilité de l'archive Windows** : elle reste construite une fois et attestée (`native/libghostty/README.md:152-170`).
- **Requalification des sessions persistantes** (`docs/release/persistent-qualification.md`) : elle n'a pas été rejouée au pin `b699ea79` et n'est pas un gate de fusion.
- **Version Rust codée en dur de la lane macOS** (`libghostty-macos.yml:128,141`) : défaut préexistant, sans lien avec Zig ni Ghostty.

## Files NOT to Modify

- `src-app/src/terminal/types.rs` et son test de garde `alacritty_is_absent_from_the_app_crate` : vocabulaire neutre du renderer, inchangé par le bump.
- `src-app/src/terminal/pty_session/session_backend.rs` (`TerminalSessionBackend`) : frontière du moteur ; le portage reste dans `crates/paneflow-terminal-ghostty`.
- `src-app/Cargo.toml`, en particulier les quatre `rev` GPUI et `features = ["font-kit"]` : hors de ce bump.
- `src-app/build.rs`, plafonds de taille des binaires auxiliaires : inchangés.
- `native/libghostty/bindings.rs` et `native/libghostty/prebuilt/**` : générés par le workflow de bump, jamais édités à la main.
- La source de Ghostty et celle de Zig : aucun patch.
- `bench/baselines/**` à la main : seulement par les scripts de baseline, sur un arbre propre.
- Les pré-releases GitHub existantes (`libghostty-vt-b699ea79…`, `libghostty-vt-0c2a290d…`, variante `-recipe-11f8a3ef`) : jamais modifiées ni supprimées.
- `crates/paneflow-home` : sans rapport avec le moteur.

## Technical Considerations

- **Architecture :** faut-il garder `GhosttyTerminalWriteStop` sur la pile à chaque `feed`, ou en champ réutilisé de `DisplayTerminal` ? Recommandé : sur la pile, puisque la bibliothèque le réinitialise à chaque appel (`ghostty/src/terminal/c/terminal.zig:1156-1157`). L'ingénierie confirme qu'aucune allocation n'apparaît au gate `terminal-alloc`.
- **Recette Windows :** `ZIG_LIB_DIR` limité à l'appel, ou copie canonique de Zig avec la bibliothèque source, comme sous macOS ? Recommandé : `ZIG_LIB_DIR`, le plus petit changement. Le repli est documenté au risque 3. À confirmer par US-003.
- **Workflow de bump :** la version de Zig peut-elle être exposée par le job `resolve` sans changer son contrat de sorties pour les autres jobs ? Recommandé : une nouvelle sortie `zig_version`, en lecture seule.
- **Données :** aucun changement de schéma. Le format de snapshot reste à la version 1 (`ghostty/src/terminal/snapshot/envelope.zig:29`), et Paneflow n'appelle jamais `decoder_next`. Faut-il poser `GHOSTTY_SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT = 0` par défense ? Recommandé : non dans ce PRD, puisque `adopt` remet la limite à 0 (`snapshot_codec.rs:203`).
- **Dépendances :** aucune dépendance Rust nouvelle. Côté libghostty, seul uucode change de commit ; simdutf, highway, wuffs, les données X.Org et la table de touches kitty sont inchangés.
- **Migration :** la compatibilité host-desktop est refusée par conception à chaque pin (`crates/paneflow-host/src/protocol.rs:135-160`). Aucune migration de données. Retour arrière : revert du commit atomique.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Avis libghostty-vt non corrigés dans le binaire livré | 3 (GHSA-qjfx, GHSA-w9q4, GHSA-jqr7) | 0 | Month-1, première release après fusion | `git merge-base --is-ancestor` de chaque correctif contre `source_sha` du manifeste livré |
| Tests Paneflow couvrant ces avis | 0 | 3, qui échouent ou plantent sur l'ancien moteur | Month-1 | `cargo test -p paneflow-terminal-ghostty --test upstream_fixes` et rapport d'US-008 |
| Archives reproductibles au bit près (Linux et macOS) | 3 sur 3 sous Zig 0.16.0 | 3 sur 3 sous Zig 0.17.0 | Month-1 | `--verify-reproducible` du workflow de bump et lane macOS |
| Endroits où la version de Zig est écrite à la main dans les workflows | 5 (`libghostty-bump.yml` x2, trois lanes) | 3 (les trois lanes) | Month-1 | `git grep -n 'ZIG_VERSION\|version: "0\.' .github` |
| Régressions attribuées au bump | N/A (nouveau) | 0 | Month-6 | Issues GitHub et passes visuelles |
| Retard du pin sur le dernier tag Ghostty | 1 tag (pin 1.3.2-dev, tag 1.4.0) | 0 tag | Month-1, puis à chaque tag | `ghostty_app_version` (`native/libghostty/manifest.toml:6`) comparé aux tags upstream |

## Open Questions

1. **Une seule session Windows ?** Faut-il faire une seule session matérielle Windows qui couvre EP-003 du PRD GPUI (`tasks/prd-gpui-bump-37faef81.md`, encore TODO) et EP-004 de ce PRD ? Arthur décide avant US-015. Si oui, l'ordre de fusion des deux PRD est à fixer.
2. **Si A1 tombe ?** Si Zig 0.17.0 casse le déterminisme Linux ou macOS, faut-il accepter une construction unique attestée, comme sous Windows, ou attendre un correctif Zig ? Arthur décide avant US-004. US-003 le révèle.
3. **Release dédiée aux GHSA ?** Faut-il une release de correctif (0.17.x) dès la fusion pour livrer les trois avis, plutôt que d'attendre la prochaine release fonctionnelle ? Arthur décide après EP-004.
4. **Sort de la PR bot :** une fois son commit repris dans le commit atomique, elle est fermée sans fusion. C'est une action externe à confirmer par Arthur à la fusion.
[/PRD]

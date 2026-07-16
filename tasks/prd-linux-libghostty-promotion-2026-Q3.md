# PRD: Migration complète vers Ghostty sur Linux - 2026-Q3

## Document Control

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 2.0 | 2026-07-16 | Arthur Jean | Décision de migration Linux actée, suppression des conditions de promotion manuelles et clôture du rollout |

## Status

**DONE depuis le 2026-07-16.** Ghostty est le backend terminal standard de Paneflow sous Linux. Cette décision produit remplace le processus de qualification préalable décrit dans les versions 1.0 et 1.1 de ce document.

Les tests, vérifications ABI, smokes de paquets, contrôles de liaison statique et checks cross-platform restent des protections techniques normales. Ils ne constituent plus une autorisation distincte pour choisir Ghostty comme backend Linux par défaut. Le rapport de promotion, son checker et les conditions temporelles ont été supprimés.

## Contexte et décision

Le backend libghostty Linux est fonctionnel dans Paneflow. Le choix produit est désormais de l'utiliser sur tous les chemins de build Linux normaux, sans attendre une période de dogfood, un nombre minimal de sessions ou un rapport manuel supplémentaire.

La migration reste strictement limitée à Linux:

- `cargo run`, `cargo build`, les tests par défaut et les artefacts Linux officiels activent `libghostty-linux`.
- `terminal.backend = auto` résout Ghostty pour chaque nouvelle session Linux standard.
- `terminal.backend = alacritty` reste un rollback explicite pour les nouvelles sessions Linux.
- macOS et Windows restent Alacritty-only, sans compilation ni liaison de libghostty.
- Une session active conserve son backend jusqu'à sa fermeture.

## Objectifs

1. Faire de Ghostty le backend effectif de toute installation ou exécution Linux standard.
2. Permettre un `cargo run` depuis un checkout propre sans Zig ni checkout Ghostty local.
3. Utiliser la même identité native épinglée dans les builds développeur, CI et release.
4. Préserver un rollback explicite vers Alacritty sous Linux.
5. Garantir l'absence de changement de backend sur macOS et Windows.

## Architecture livrée

`paneflow-app` expose `libghostty-linux` comme feature par défaut. Les dépendances natives restent sous `cfg(target_os = "linux")`, donc cette feature est inerte sur macOS et Windows. Les workflows non-Linux utilisent en plus `--no-default-features` pour rendre cette frontière explicite.

Les archives statiques vérifiées pour `x86_64-unknown-linux-gnu` et `aarch64-unknown-linux-gnu` sont versionnées sous `native/libghostty/prebuilt/<target>`. Leur hash est épinglé par cible dans le manifest. `paneflow-libghostty-sys` sélectionne les entrées natives dans cet ordre:

1. `PANEFLOW_LIBGHOSTTY_DIR`, pour une archive préparée explicitement par la CI ou un mainteneur.
2. `native/libghostty/prebuilt/<target>`, pour tout build Linux standard depuis un checkout propre.

`build.rs` ne télécharge rien, ne lance ni Zig ni le build Ghostty, et ne dépend d'aucune commande POSIX externe.

## Epic

### EP-005: Basculer Paneflow Linux sur Ghostty

**Status:** DONE

**Definition of Done:** Ghostty est actif par défaut sur tous les chemins Linux standards, les paquets le lient statiquement, Alacritty reste sélectionnable explicitement sous Linux, et les autres OS restent Alacritty-only.

#### US-018: Utiliser Ghostty par défaut sur Linux

**Status:** DONE  
**Priority:** P0  
**Size:** M (3 pts)

**Acceptance Criteria:**

- [x] Given un checkout propre sur Linux x86_64 ou ARM64, when `cargo run` ou `cargo build` est exécuté sans option, then `libghostty-linux` est actif sans Zig ni source Ghostty locale.
- [x] Given une nouvelle session Linux avec `terminal.backend = auto`, when elle démarre, then Ghostty est résolu et les diagnostics conservent `requested=Auto resolved=ghostty`.
- [x] Given `terminal.backend = alacritty` sous Linux, when une nouvelle session démarre, then Alacritty est utilisé sans migration de données ni redémarrage global.
- [x] Given une session déjà active, when la configuration backend change, then la session conserve son moteur et seules les nouvelles sessions utilisent la nouvelle valeur.
- [x] Given une release Linux x86_64 ou ARM64, when le workflow construit et package Paneflow, then il génère ou sélectionne l'archive épinglée, la lie statiquement et inclut les notices natives.
- [x] Given macOS ou Windows, when Paneflow compile ou démarre avec `terminal.backend = auto`, then Alacritty reste le backend et aucune dépendance native Ghostty n'entre dans le graphe de build.
- [x] Given un build Linux explicitement lancé avec `--no-default-features`, when une session utilise `auto`, then Alacritty reste disponible comme configuration de développement ou de diagnostic.
- [x] Given un échec Ghostty avant spawn avec `auto`, when le fallback est possible, then Alacritty démarre exactement un child et une raison actionnable est journalisée.
- [x] Given un échec après spawn du PTY Ghostty, when la session échoue, then aucun second shell Alacritty n'est créé.
- [x] Given la documentation utilisateur et release, when la migration est livrée, then elle présente Ghostty comme défaut Linux et ne l'annonce pas sur macOS ou Windows.

## Functional Requirements

- FR-01: Ghostty DOIT être le backend par défaut des builds et artefacts Linux standards.
- FR-02: Un checkout propre Linux DOIT compiler avec les archives statiques versionnées sans outil natif supplémentaire.
- FR-03: La CI Linux PEUT remplacer les archives versionnées par une archive générée depuis le SHA épinglé via `PANEFLOW_LIBGHOSTTY_DIR`.
- FR-04: `terminal.backend = alacritty` DOIT rester fonctionnel sous Linux pour les nouvelles sessions.
- FR-05: macOS et Windows DOIVENT rester Alacritty-only et ne DOIVENT pas résoudre les dépendances natives Ghostty.
- FR-06: Le changement de configuration NE DOIT PAS migrer une session active entre moteurs.
- FR-07: Les paquets Linux DOIVENT lier libghostty statiquement et inclure les notices tierces.
- FR-08: `build.rs` NE DOIT PAS télécharger de source ni exécuter Zig.

## Validation continue

Le workflow libghostty Linux conserve les vérifications reproductibles, ABI, tests debug et release, corpus différentiel, fuzz, stress PTY, paquets multi-distribution, notices et liaison statique. Le workflow release reconstruit l'archive depuis le SHA épinglé et l'utilise directement. Ces contrôles détectent les régressions de code ou de packaging, sans réintroduire de rapport d'approbation produit.

La vérification Fedora Wayland et X11/XWayland reste un runbook manuel utile lorsqu'un changement touche le rendu, l'input, l'IME ou le clipboard. Elle n'est pas une condition temporelle de migration.

## Rollback

Définir `terminal.backend = alacritty`, puis créer un nouveau terminal. Les sessions existantes gardent leur backend. Le retrait futur du code Alacritty sous Linux nécessitera une décision et un scope séparés.

## Non-Goals

- Activer Ghostty sur macOS ou Windows.
- Supprimer immédiatement le backend Alacritty sous Linux.
- Basculer à chaud une session existante.
- Télécharger ou compiler Ghostty depuis `build.rs`.
- Ajouter des capacités terminal visibles sans rapport direct avec la migration.

## Success Metrics

| Metric | Target |
|--------|--------|
| Défaut Linux | 100 % des nouvelles sessions `auto` des builds standards utilisent Ghostty |
| Exécution développeur | `cargo run` fonctionne depuis un checkout propre sans Zig |
| Artefacts Linux | x86_64 et ARM64 utilisent une liaison statique vérifiée |
| Rollback Linux | `terminal.backend = alacritty` reste fonctionnel pour les nouvelles sessions |
| Non-régression OS | macOS et Windows restent Alacritty-only |

## Open Questions

Aucune question ne bloque cette migration. L'activation de Ghostty sur macOS ou Windows et la suppression définitive d'Alacritty sont des décisions ultérieures, chacune avec son propre scope.

[/PRD]

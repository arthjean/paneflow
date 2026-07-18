# Brouillon post LinkedIn — v0.4.3 (terminal UX)

Note pour relecture : d'après CHANGELOG.md:82 (section [0.4.2]) + tags git (v0.4.2 = 85c00c4) + commits (803b476 "feat(terminal): US-017/US-018/US-019 — match rail, fleet grep, font zoom" arrive après v0.4.2 et est inclus dans la release 0.4.3 du 2026-06-11), les deux features demandées (per-pane font zoom + recherche Ctrl+Shift+F avec ses améliorations rail + fleet) sont dans 0.4.3. Le 0.4.2 était uniquement le refresh logo + artwork. J'ai rédigé le texte avec les faits exacts (changelog + keybindings/defaults.rs + terminal/{view,search}.rs + element/font.rs). Pas de storytelling après le hook. Ton direct / peer. Raccourcis recoupés avec les defaults et le tableau README.

---

Vous passez encore 20 secondes à scroller pour retrouver une erreur dans 8000 lignes, ou vous zoomez toute la fenêtre parce que 14 px est illisible sur un long run ?

Paneflow 0.4.3 règle le problème à la source dans le terminal.

**Zoom / dézoom par pane (indépendant)**

Ctrl+= / Ctrl+- / Ctrl+0 (Cmd sur macOS).

- Pas de 1 px, clamp 8–32 px.
- Seul le pane focusé change. Les siblings restent à leur échelle.
- Le grid PTY reflow complet (colonnes/lignes recalculés + notify resize au shell, exactement comme un resize de fenêtre).
- Override stocké par pane dans la session. Reset (Ctrl+0) repasse le pane en "suit le réglage global" sans le figer.

**Recherche dans le buffer (Ctrl+Shift+F)**

Barre locale classique : saisie, navigation (Entrée = suivant, Maj+Entrée = précédent), regex (Alt+R), Esc pour refermer.

Nouveautés de la release :
- Rail de matches sur la scrollbar : chaque hit est un tick sur la piste (décimé au pixel près — 10 000 matches coûtent le même rendu que 10). Click sur la piste = jump proportionnel comme avant.
- Fleet grep (Alt+F ou toggle "Fleet" dans la barre) : la requête part sur tous les panes de tous les workspaces (exécuté hors render thread). Liste les panes avec leur count de hits, badges flash sur les tabs. Entrée = warp + recherche locale pré-armée sur la cible.

Vidéo démo (quelques secondes) qui montre les deux en action.

0.4.3 est dispo (Linux/macOS, Windows en cours). Releases + changelog : https://github.com/arthjean/paneflow/releases

---

# Variante ultra-courte (si tu veux coller direct sous la vidéo)

Ctrl+= / Ctrl+- / Ctrl+0 : zoom police indépendant par pane (8-32 px, reflow PTY, persisté).

Ctrl+Shift+F : recherche locale + rail de matches sur la scrollbar + Alt+F = fleet grep cross-workspace avec téléport + pre-arm.

0.4.3. Vidéo ci-dessous.
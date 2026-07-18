# Brouillon Show HN - Paneflow

Document de travail. On construit le post étape par étape, section par section.

## Titre

```
Show HN: Paneflow - cross-platform GPUI app for parallel coding agents
```

70 caractères (limite Hacker News : 80). Angle retenu : app native cross-platform en GPUI pour piloter plusieurs agents en parallèle.

## Texte du post

### Version FR

Je fais tourner beaucoup d'agents de code en parallèle sur mes projets, notamment Claude Code, Codex, OpenCode ou d'autres CLI. Les lancer n'a jamais été le problème. Le vrai problème, c'était de garder une vue fiable de tout ce qui tourne : lequel réfléchit, lequel attend une réponse, lequel a fini, sur quelle branche.

Dans une grille tmux, ou dans une dizaine de fenêtres Ghostty avec plusieurs sessions dans chacune, la charge mentale monte vite. À un moment, ce n'est plus un problème de terminal : c'est un problème de coordination. Ghostty est très bien, mais chaque fenêtre relance son propre renderer GPU, donc cette façon de travailler finit aussi par peser sur la machine.

Alors je l'ai construit. Paneflow, un workspace natif en Rust sur GPUI, le framework de Zed. Il tourne aujourd'hui sur Linux, macOS Apple Silicon et Windows x64 ; macOS Intel et Windows ARM64 ne sont pas encore livrés. Pas de WSL, pas d'Electron. Il est pensé pour alléger à la fois la charge mentale et la charge machine. Sur ma machine, juste lancé, Paneflow tourne autour de 50 Mo de mémoire et 0,2 % CPU, contre environ 963 Mo et 0,6 % CPU pour Codex App.

Le vrai pari est la coordination. J'ai appelé ce système Paneflow Conductor : pour le moment, ce que j'expose aux agents passe volontairement dans l'interface CLI. Les actions de coordination dont j'ai besoin passent par la CLI publique et le socket local, tandis que la GUI reste l'endroit où je supervise et reprends la main sur n'importe quelle pane. Je peux tout piloter à la main, ou laisser un agent en coordonner trois autres sous supervision. Quand plusieurs agents travaillent sur la même branche, ils ne restent pas chacun dans leur coin : Paneflow détecte les changements du repo, expose l'état de chaque pane, et permet à un agent de lire la sortie d'un autre avant d'agir pour réduire les chevauchements.

Si tu connais les Agent Teams de Claude Code ou les workflows de type swarm, le modèle mental est proche : un leader qui délègue. La différence, c'est que Paneflow Conductor n'est pas un swarm fermé dans un seul outil. Il transforme chaque pane en terminal pilotable et observable. Par exemple Claude Code peut piloter un pane Codex CLI, OpenCode, Grok Builder, ou n'importe quelle autre CLI qui tourne dans un terminal.

`paneflow ps` liste les panes et agents en cours avec leur état réel. `paneflow watch` streame les changements en JSONL, poussés par les hooks et l'event bus, sans polling. Un agent peut donc voir ce qui tourne, lire l'état d'un pane, envoyer un prompt à un autre agent et attendre un event, le tout via la CLI publique.

Par défaut, les prompts sont pré-remplis et c'est moi qui appuie sur Entrée. Le mode auto-submit est opt-in, et les lectures inter-panes restent encadrées par une protection anti-injection qui traite la sortie terminal comme non fiable.

Les agents peuvent aussi lire le terminal des autres en lecture seule via le serveur MCP intégré (`list_panes`, `read_pane`, `search_pane`). Claude Code dans un pane peut lire la sortie de test que Codex vient de produire dans un autre, sans copier-coller.

Quand je veux isoler le travail, chaque tâche peut tourner dans son propre worktree git, et Paneflow affiche les diffs côte à côte dans la vue Review.

Gratuit et open source (GPL-3.0-or-later), pensé pour les power users qui pilotent plusieurs agents, pas pour remplacer ton éditeur.

Demo: https://www.youtube.com/watch?v=hElqzB2XMn0
Download: https://paneflow.dev/download
Longer write-up: https://paneflow.dev/blog/show-hn-launch
Repo: https://github.com/arthjean/paneflow

### Version EN (à poster)

I run a lot of coding agents in parallel on my projects, mostly Claude Code and Codex, sometimes OpenCode or Gemini. Starting them was never the hard part. Keeping a reliable view of everything running was: which one is thinking, which one is waiting on me, which one finished, which branch it is on, and how to act on that without scraping scrollback or polling a terminal.

In a tmux grid, or worse a dozen Ghostty windows with a few sessions in each, I kept losing context. I love Ghostty, but each separate window brings its own GPU renderer, so that workflow also gets heavy fast.

So I built Paneflow: a native workspace in Rust on GPUI, the UI framework behind Zed. It runs on Linux, macOS Apple Silicon, and Windows x64 today; macOS Intel and Windows ARM64 are not shipped yet. No WSL, no Electron. On my machine, freshly launched, Paneflow sits around 50 MB of memory and 0.2% CPU, compared with roughly 963 MB and 0.6% CPU for Codex App.

The bigger bet is coordination. I called this system Paneflow Conductor. For now, agents talk to Paneflow through the public CLI and local socket by design, while the GUI stays where I supervise the run and take over any pane. I can drive everything by hand, or let one agent coordinate three others under supervision. It also makes same-branch multi-agent work less blind: each agent can see the other panes' state, read their output, and react to changes landing in the repo.

If you have used Claude Code Agent Teams or swarm-style workflows, the mental model is similar: a lead agent delegates. The difference is that Paneflow Conductor is not a closed swarm inside one tool. It turns every pane into a controllable, observable terminal. For example, Claude Code can drive a Codex CLI pane, OpenCode, Grok Builder, or any other CLI that runs in a terminal.

`paneflow ps` lists the running panes and agents with their real state. `paneflow watch` streams state changes as JSONL, pushed by hooks and the event bus instead of polling. An agent can see what is running, read a pane's current state, send a prompt to another agent, and wait for an event, all through the public CLI.

By default, prompts are pre-filled and I still press Enter. Auto-submit is opt-in, and cross-pane reads stay wrapped as untrusted terminal output so a repo or agent transcript cannot silently hijack the run.

Agents can also read each other's terminals through the built-in read-only MCP bridge (`list_panes`, `read_pane`, `search_pane`). Claude Code in one pane can read the test output Codex just produced in another without me copy-pasting between windows.

When I want isolation, each task can run in its own git worktree, and Paneflow shows the resulting diffs side by side in the Review view.

Free and open source (GPL-3.0-or-later).
Demo: https://www.youtube.com/watch?v=hElqzB2XMn0
Download: https://paneflow.dev/download
Longer write-up: https://paneflow.dev/blog/show-hn-launch
Repo: https://github.com/arthjean/paneflow

### Notes de mesure RAM (méthode + mesures)

**Claim à citer (lancement à vide, même machine, app juste ouverte) :**

| App | Mémoire | CPU |
|---|---:|---:|
| Paneflow | ~50 Mo | ~0,2 % |
| Codex App | ~963 Mo | ~0,6 % |

**Formulation EN courte :** freshly launched on the same machine, Paneflow sits around 50 MB of memory and 0.2% CPU, compared with roughly 963 MB and 0.6% CPU for Codex App.

**Cadrage honnête (HN vérifiera) :** c'est un comparatif à froid, app juste lancée, pas un benchmark de charge réelle avec agents actifs. À garder comme preuve de légèreté au repos, et à accompagner d'une capture/méthode de mesure le jour J si quelqu'un demande.

## Réponses aux objections (préparées pour le jour J)

> Règle de ton : concéder quand l'objection est juste (HN récompense l'honnêteté, pas la défense), répondre en pair, jamais marketing, jamais défensif. Court.

### 1. Pourquoi pas tmux / Zellij ?

**FR :** tmux et Zellij sont excellents, surtout en SSH et en headless, et je m'en sers encore là. Paneflow est une app GUI locale, pas un multiplexeur que tu portes sur un serveur distant. Ce que j'ajoute par-dessus le multiplexage : un statut d'agent lisible (alimenté par les hooks de chaque CLI, pas du polling), la lecture inter-panes via MCP, et la review de diffs/worktrees au même endroit. Si ton travail est surtout distant, reste sur tmux ou Zellij, c'est le bon outil.

**EN:** tmux and Zellij are great, especially over SSH and headless, and I still use them there. Paneflow is a local GUI app, not a multiplexer you carry onto a remote box. What it adds on top of multiplexing: a readable per-agent status (driven by each CLI's hooks, not polling), cross-pane reads over MCP, and diff/worktree review in the same place. If your work is mostly remote, tmux or Zellij are still the right tool.

### 2. Pourquoi pas juste les splits de mon terminal (Ghostty / Kitty / WezTerm) ?

**FR :** Honnêtement, face aux splits dans une seule fenêtre, mon argument RAM tombe : tu paies déjà un seul renderer. L'écart se creuse uniquement contre l'habitude de N fenêtres séparées, chacune avec son renderer GPU. Contre des splits, les vraies différences sont ailleurs : la sidebar de statut hook-driven, le pont MCP entre panes, et la vue de review worktree-par-worktree. Si tes splits te suffisent et que tu suis tes agents de tête, tu n'as pas besoin de Paneflow.

**EN:** Honestly, against splits in a single window my RAM argument doesn't hold: you already pay for one renderer. The gap only shows against the habit of N separate windows, each with its own GPU renderer. Against splits, the real differences are elsewhere: the hook-driven status sidebar, the MCP bridge between panes, and the worktree-by-worktree review view. If your splits already work and you track your agents in your head, you don't need Paneflow.

### 3. En quoi c'est différent de cmux / Conductor / [autre orchestrateur d'agents] ?

**FR :** La plupart de ceux que j'ai testés sont mac-only ou des apps web/Electron. Mon angle, c'est des binaires natifs sur Linux, macOS Apple Silicon et Windows x64 aujourd'hui, sans WSL ; macOS Intel et Windows ARM64 ne sont pas encore livrés. Les deux choses que je n'ai pas trouvées ailleurs : les agents qui se lisent entre eux via MCP, et un moteur de flow scriptable (`flow.toml`) pour enchaîner des étapes d'agents. Je ne prétends pas réinventer l'orchestration ; je voulais juste qu'elle soit native et cross-platform.

**EN:** Most of the ones I tried are macOS-only or web/Electron apps. My angle is native binaries for Linux, macOS Apple Silicon, and Windows x64 today, with no WSL; macOS Intel and Windows ARM64 are not shipped yet. The two things I couldn't find elsewhere: agents reading each other over MCP, and a scriptable flow engine (`flow.toml`) to chain agent steps. I'm not claiming to reinvent orchestration; I just wanted it native and cross-platform.

### 4. Un pont MCP qui laisse un agent lire la sortie d'un autre pane, ce n'est pas un vecteur de prompt-injection ?

**FR :** Bonne question, et oui c'est un vrai sujet. Trois précisions. Le pont est strictement read-only : `list_panes`, `read_pane`, `search_pane`, aucun outil n'écrit ni ne pilote un pane. La sortie terminal est traitée explicitement comme non fiable côté serveur. Et le risque est exactement celui que tu prends déjà quand tu copies-colles la sortie d'un agent dans un autre, sauf qu'ici c'est explicite et borné à de la lecture. Ça ne supprime pas le risque que le modèle obéisse à du texte hostile lu dans un buffer, c'est au modèle de ne pas le faire, mais ça ne lui donne aucun pouvoir d'écriture.

**EN:** Fair question, and yes it's a real concern. Three things. The bridge is strictly read-only: `list_panes`, `read_pane`, `search_pane`, no tool writes to or drives a pane. Terminal output is explicitly treated as untrusted on the server side. And the risk is exactly the one you already take when you copy-paste one agent's output into another, except here it's explicit and bounded to reads. It doesn't remove the risk of a model obeying hostile text it reads from a buffer, that's on the model, but it grants no write power.

### 5. Un seul process rend tout : s'il crashe, je perds tous mes agents ?

**FR :** C'est le tradeoff d'un renderer mutualisé, je ne vais pas le cacher. Si le process meurt, les PTY enfants meurent avec lui, comme quand ton terminal crashe. Deux choses limitent la casse : la session est persistée (workspaces, layouts, CWD) et restaurée au redémarrage, et les agents comme Claude Code ou Codex ont leur propre reprise de session. Ce que tu perds, c'est l'état en vol d'un tour en cours, pas ta disposition de travail.

**EN:** That's the tradeoff of a shared renderer, I won't hide it. If the process dies, the child PTYs die with it, same as when your terminal crashes. Two things limit the blast radius: the session is persisted (workspaces, layouts, CWD) and restored on restart, and agents like Claude Code or Codex have their own session resume. What you lose is the in-flight state of a running turn, not your working layout.

### 6. Télémétrie ? Ça appelle la maison ?

**FR :** Pas par défaut. La télémétrie est opt-in via un modal au premier lancement, désactivée tant que tu ne dis pas oui. Si tu actives, elle envoie cinq events de cycle de vie (lancement, sortie, check et install d'update, session corrompue), jamais de contenu terminal, de chemins ni de prompts. `PANEFLOW_NO_TELEMETRY=1` est un kill switch inconditionnel, et le client entier est auditable dans `crates/paneflow-telemetry/`. En pratique je ne m'en sers même pas pour de l'analytics produit, mon seul analytics est sur le site web.

**EN:** Not by default. Telemetry is opt-in via a first-run modal, off until you say yes. If you enable it, it sends five lifecycle events (start, exit, update check and install, session corrupted), never terminal content, paths, or prompts. `PANEFLOW_NO_TELEMETRY=1` is an unconditional kill switch, and the whole client is auditable in `crates/paneflow-telemetry/`. In practice I don't even use it for product analytics, my only analytics is on the website.

### 7. Pourquoi GPL-3.0 ? Je peux l'utiliser au boulot ?

**FR :** Oui, utilise-le au boulot sans problème : la GPL ne contraint que la redistribution d'une version modifiée, pas l'usage. J'ai choisi le copyleft volontairement pour que les forks restent ouverts. C'est un outil que je veux voir rester libre, pas une lib que tu embarques dans un produit fermé.

**EN:** Yes, use it at work freely: GPL only constrains redistributing a modified version, not usage. I chose copyleft on purpose so forks stay open. It's a tool I want to keep free, not a library you'd vendor into a closed product.

### 8. Pourquoi GPUI, et c'est stable, surtout sur Windows ?

**FR :** GPUI est le framework d'interface derrière Zed, donc il tourne déjà en prod sur les trois plateformes, rendu GPU natif (Vulkan, Metal, DirectX). C'est un pari assumé : je le consomme depuis git, pas depuis crates.io. Côté Paneflow, les builds livrés aujourd'hui couvrent Linux, macOS Apple Silicon et Windows x64 ; macOS Intel et Windows ARM64 ne sont pas encore livrés. Sur Windows précisément, le MSI signé est live aujourd'hui ; les limitations connues sont listées dans `docs/WINDOWS.md`. La latence ressentie des apps Electron/Tauri que je fuyais vient du WebView, GPUI n'en a pas.

**EN:** GPUI is the UI framework behind Zed, so it already runs in production on all three platforms with native GPU rendering (Vulkan, Metal, DirectX). It's a deliberate bet: I consume it from git, not crates.io. For Paneflow, the shipped builds today cover Linux, macOS Apple Silicon, and Windows x64; macOS Intel and Windows ARM64 are not shipped yet. On Windows specifically, the signed MSI is live today; known limitations are in `docs/WINDOWS.md`. The Electron/Tauri latency I was running from comes from the WebView, GPUI has none.

### 9. Ça me verrouille sur quels agents ?

**FR :** Aucun. Un pane est un vrai terminal, donc n'importe quel CLI tourne dedans, y compris un que je n'ai jamais vu. Ce qui est par-agent, c'est le confort : le statut hook-driven et `mcp install` couvrent Claude Code, Codex, Gemini et opencode aujourd'hui, plus d'autres via des shims. Lance autre chose et il tourne quand même, tu perds juste le statut riche et l'install MCP automatique.

**EN:** None. A pane is a real terminal, so any CLI runs in it, including one I've never seen. What's per-agent is the comfort layer: the hook-driven status and `mcp install` cover Claude Code, Codex, Gemini and opencode today, plus others via shims. Run something else and it still works, you just lose the rich status and the automatic MCP install.

### 10. Open source aujourd'hui, mais quel est le business model ? Ça restera gratuit ?

**FR :** Le core que tu vois reste OSS et gratuit pour toujours, sous GPL. Je pars sur un modèle open-core à la VSCode : si je monétise un jour, ce sera des features autour (collaboration, cloud, ce genre de choses), jamais en rognant ce qui est déjà là. Je ne vais pas te promettre que je ne facturerai jamais rien, ce serait malhonnête, mais le noyau ne deviendra pas payant.

**EN:** The core you see stays OSS and free forever, under GPL. I'm going with an open-core model like VSCode: if I ever monetize, it'll be features around it (collaboration, cloud, that kind of thing), never by walling off what's already there. I won't promise I'll never charge for anything, that'd be dishonest, but the core won't go paid.

### 11. Comment tu calcules le coût en dollars ?

**FR :** C'est une estimation, pas une facture : tokens consommés par l'agent, multipliés par une table de prix par modèle que je tiens dans le binaire. Utile pour comparer le coût relatif entre worktrees et repérer un agent qui part en boucle, pas pour réconcilier au centime avec ta facture provider.

**EN:** It's an estimate, not a bill: tokens consumed by the agent, multiplied by a per-model price table I keep in the binary. Useful to compare relative cost across worktrees and catch an agent that's looping, not to reconcile to the cent with your provider invoice.

### À vérifier avant de poster (claims qui touchent ces réponses)

- **Latence (objection 8)** : "latency I could feel" est subjectif et c'est ok de le dire tel quel. Ne pas claim de chiffre comparatif Electron vs GPUI sans benchmark. La sonde `PANEFLOW_LATENCY_PROBE` existe mais n'a pas de chiffre publié.
- **Coût en dollars** : FAIT (objection 11 + corps cadré comme "estimation"). Garder le cadrage "estimate, not a bill".
- **Crash behavior (objection 5)** : confirmer que la session restore relance bien layouts+CWD et pas les process agents (c'est ce que je décris, à re-tester une fois avant le jour J).
- **GPL** : FAIT, les deux versions du post disent `GPL-3.0-or-later`.
- **RAM** : FAIT côté chiffre à citer (lancement à vide : Paneflow ~50 Mo / 0,2 % CPU ; Codex App ~963 Mo / 0,6 % CPU). Reste à garder une capture ou la méthode de mesure prête pour le jour J.

## Checklist jour J (timing, démo, dispo)

### Timing

- **Jour** : mardi, mercredi ou jeudi. Éviter lundi (noyé) et vendredi/week-end (audience basse).
- **Heure optimale** : viser **18h30 heure française (CEST)**, soit **9h30 Pacific Time**. Fenêtre de sécurité : **18h-19h CEST**. C'est le meilleur compromis si tu peux rester disponible tard : l'audience US est réveillée, l'Europe est encore en ligne, et tu gardes plusieurs heures pour répondre.
- **Pourquoi cette fenêtre** : un post entre par la "new queue" (la file des nouveautés). Il atteint la front page seulement s'il accumule assez d'upvotes vite : la vélocité des 1-2 premières heures pèse plus que le total. Poster vers 9h-10h Pacific maximise les chances de capter l'audience US au moment où elle est déjà active, sans tomber trop tard dans la journée.

### Pré-vol (la veille / le matin)

- [ ] Repo, site et README alignés (FAIT).
- [ ] Social preview GitHub uploadée (Settings > General), testée en collant le lien repo dans un champ X/LinkedIn.
- [ ] Démo vidéo uploadée, lien testé en navigation privée.
- [ ] Titre final tranché (angle cross-platform GPUI app, cf section Titre).
- [ ] Chiffres RAM/CPU à froid Paneflow vs Codex App capturés (section Notes RAM), screenshot prêt.
- [ ] Premier commentaire prêt à coller (clarifications + méthodo RAM).
- [ ] Liens du post testés un par un (repo, download, démo).

### Démo (décidé : muette + annotations texte, pas de voix off)

Raison : voix off anglaise non-native = risque downside élevé sur HN (lecture audible, accent au premier plan) pour un gain faible. Le muet annoté joue sur la force (anglais écrit nickel), marche sans le son (majorité des viewers), et est plus dense. Voix réservée à X/LinkedIn plus tard si besoin.

- **Capture** : OBS Studio. Sur Wayland (Fedora), source = **Screen Capture (PipeWire)** (la capture X11 classique ne marche pas) ; le portail GNOME laisse choisir la fenêtre Paneflow. Cadrer sur Paneflow (fenêtre seule ou plein écran maximisé), 1920x1080, 30 fps (60 si on veut les animations GPUI bien fluides), sortie mp4.
- **Montage** : kdenlive ou DaVinci Resolve (gratuits). Couper les temps morts pour tenir 60-90s, commencer direct dans l'action (pas d'intro), poser 4-5 annotations texte courtes en anglais : "Claude Code + Codex, same repo", "sidebar shows who's waiting on you", "one agent reads another's pane (MCP)", "review every worktree side by side".
- **Hébergement** : YouTube en **public/répertorié** (PAS unlisted). On veut la découverte YouTube gratuite (recherche + recommandations) et le compteur de vues comme preuve sociale. Référence : la vidéo cmux/Manaflow "terminal built for multitasking" est à 30k vues, en partie grâce au public. Coller l'URL dans le `[lien vidéo]` du post ET en haut du repo.
- **Packaging YouTube** (compte car public) : titre orienté recherche ("Paneflow: run Claude Code, Codex and any coding agent in parallel"), description avec liens repo + site + download + 2-3 lignes de pitch, thumbnail lisible et annotée. Chaîne : envisager une chaîne "créateur" unique (Paneflow + Distill + Rust Doctor) plutôt qu'une chaîne par produit, comme Manaflow regroupe ses produits sous @ManaflowAI.
- **Bonus** : régénérer `assets/images/demo.gif` à partir de la nouvelle vidéo si l'UI a bougé. Le GIF actuel est `960x540`, 8,39 MiB, et date du 12/06.

### Déroulé de soumission

1. Sur news.ycombinator.com/submit : **title** = `Show HN: Paneflow - cross-platform GPUI app for parallel coding agents`, **url** = `https://github.com/arthjean/paneflow` (le repo : stars + audience dev), **text** = vide (si l'URL est remplie, le champ texte n'est pas utilisé).
2. **Juste après**, poster la **Version EN (à poster)** en **premier commentaire** d'auteur, avec le lien démo YouTube + le lien download dedans.
3. Le post arrive dans /newest, invisible du grand public à ce stade.
4. Monter en front page (home, top ~30, le trafic) = upvotes rapides + peu de flags. L'algo favorise la vélocité des 1-2 premières heures, pas le total. D'où le créneau.
5. Si ça prend : pic de clics/stars/downloads. Si ça ne prend pas : un repost espacé est toléré si le 1er n'a eu aucune attention.
6. Rappel : un Show HN est one-shot. Ne pas poster si la vidéo n'est pas prête ou si les liens ne sont pas testés ; décaler à mardi plutôt que bâcler.

### Dispo (le facteur n°1 de survie d'un Show HN)

- Bloquer **au moins 5h juste après le post** pour répondre dans le thread sans délai. Un auteur présent et réactif est le plus gros signal positif : ça nourrit la discussion, donc la visibilité, donc les upvotes.
- Garder les réponses aux objections (section au-dessus) sous la main, mais reformuler à la main, pas copier-coller mot pour mot (HN sent le texte préfabriqué).
- Ton : concéder quand l'objection est juste, jamais défensif, parler en pair.

### Interdits HN (à ne pas faire)

- Ne **jamais** demander d'upvotes (ni publiquement ni en privé) : c'est contre les règles, ça peut faire flag ou sanctionner le post.
- Pas de "edit: thanks for the upvotes" ni de méta-commentaire sur le score.
- Pas de comptes amis qui upvotent en rafale depuis la même IP (détecté, pénalisé).

### Après

- Répondre à tout commentaire les 2 premières heures.
- Si ça prend, relayer sur X (build-in-public, EN) et LinkedIn (FR, ton terrain). Ne pas quémander d'upvotes depuis ces canaux ; raconter le lancement, c'est tout.
- **Article de blog paneflow.dev** : PAS un prérequis du Show HN (HN pointe vers le repo). À écrire en J+1 ou après, comme contenu durable (SEO/AEO) et surtout comme pivot du post LinkedIn FR. Ne pas s'y disperser le jour du lancement.

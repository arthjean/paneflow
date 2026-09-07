# Proposition exécutable de croissance des workers Mesa

Statut : expérience intégrée, parcours natif non exécuté. Le harness copie son runtime et ses fixtures avant toute modification.

## Exécution différée

Le mode par défaut affiche NOT_EXECUTED et ne crée aucun fichier. Ajouter --run lance réellement l'expérience. Tous les arguments de périphériques doivent reprendre ceux du runner AMD Wayland déjà vérifié; aucun périphérique par défaut n'est inventé.

```sh
python3 /home/arthur/dev/paneflow-browser/native/browser/experiments/tsync/mesa-cache-growth/run-cache-growth.py \
  --source-workspace /absolute/source/workspace \
  --binary /absolute/release/paneflow \
  --host /absolute/release/paneflow-browser-host \
  --render-node /dev/dri/AMD_RENDER_NODE \
  --card /dev/dri/AMD_CARD \
  --egl-vendor /absolute/mesa-vendor.json \
  --gpui-device AMD_DEVICE_ID \
  --output /absolute/new/evidence-directory
```

La source est lue seulement. Le harness copie les scripts de qualification, fixtures, manifest et runtime CEF dans output/workspace, puis adapte seulement cette copie. La copie physique du runtime peut prendre plusieurs Go. Aucun hardlink ou symlink vers le runtime d'origine n'est créé: fetch-browser.py --verify-only réécrit verified-manifest.sha256. Les binaires applicatif/host fournis sont exécutés en place, sans être modifiés. Ils doivent correspondre au build contenant patch0008; les hashes sont archivés, mais le harness ne fabrique pas leur provenance de compilation.

L'expérience reprend run-wayland.py et son compositor isolé. La seule adaptation du runner est un handler SIGTERM/SIGINT permettant son bloc finally et le nettoyage des groupes de processus qu'il a lui-même créés. Le harness ne tue jamais un PID découvert dans /proc, ni une session extérieure. Un cleanup qui ne termine pas est signalé comme INCOMPLETE. Aucun privilège supplémentaire, ptrace, outil de profilage ou dépendance Python n'est ajouté. Les prérequis natifs existants restent Python, Bun, bwrap, dbus-run-session, Mutter et les binaires vérifiés.

## Gate et fixture

La fixture crée WebGL2 et valide un programme de préparation avant de s'armer. Elle effectue ensuite uniquement des GET vers /shader-gate/<nonce> sur sa propre origine. Le serveur isolé publie un reçu armed atomique et retourne 202. Le harness attend ce reçu, un host_ready unique et un frame, puis établit la baseline complète du GPU et de son ascendance. Après deux ticks système de marge, il publie release.json avec hash de baseline, nonce et CLOCK_MONOTONIC. Le serveur archive observed.json avant de retourner 204.

Après cette réponse, la fixture compile 120 programmes uniques en deux lots de 60. Chaque programme conserve un oracle readPixels limité à un pixel. Aucun readback complet de framebuffer n'est ajouté. Les 120 programmes expérimentaux sont distincts du programme initial. La fixture garde les titres ARMED, STARTED, PASSED:120 et FAILED pour l'archive native.

Le cache neuf est créé avant tout processus graphique, vide, propre à cette exécution, avec caches Mesa/NVIDIA activés. Le compositor et GPUI héritent aussi de ce cache: les fichiers finaux prouvent des écritures dans ce répertoire, pas leur attribution individuelle au GPU CEF. La croissance des workers est observée exclusivement dans le GPU descendant du host.

## Observation native et preuve bornée

proc_observer.py résout le GPU par son rôle déclaré ou VizCompositorTh, exige un candidat unique et une chaîne de parents vérifiée jusqu'au host, avec pid/start_ticks. La baseline lit chaque thread avec stat avant/après, nom, Seccomp, NoNewPrivs et Seccomp_filters. Les inventaires TID encadrant la collecte doivent être identiques, puis les identités sont relues. Tous les threads doivent être Seccomp=2, NNP=1 et avoir au moins un filtre. Une baseline illisible ne libère pas la gate.

L'observer ne parcourt ensuite que ce GPU, toutes les 20 ms pendant 20 secondes. Limites: 8192 processus pour la découverte, 256 threads GPU, 128 MiB lus dans /proc, 16 MiB d'événements, 1500 échantillons et 128 erreurs. Les disparitions observées sont archivées; une limite ou une erreur d'identité arrête la preuve. Une collecte de sécurité complète est refaite à la confirmation et à la fin.

Un worker confirmé doit avoir un couple tid/start_ticks absent de tous les threads initiaux, un suffixe disk$N dont N dépasse les indices initiaux, et un tick de naissance strictement après la fin de baseline en CLOCK_BOOTTIME. Les anciens workers disk doivent encore exister simultanément et leur nombre total doit augmenter. Deux observations sécurisées de la même nouvelle identité sont requises. Un thread ancien renommé, un nouveau disk$0 d'un autre cache, une naissance dans le tick frontière ou un worker déjà disparu ne sont pas acceptés. Toute sécurité insuffisante observée sur un nouveau disk worker arrête la preuve.

Les noms réels incluent un préfixe, par exemple paneflo:disk$0. Les horodatages CLOCK_MONOTONIC et les start_ticks sont archivés comme chaînes décimales. CLOCK_BOOTTIME sert exclusivement à la borne de naissance avec SC_CLK_TCK. La collecte /proc est une série de lectures vérifiées, pas un instantané atomique; elle ne prétend pas observer tous les threads transitoires entre deux polls.

## Verdict

PASS_BOUNDED_GROWTH exige la confirmation native, la réussite des 120 programmes, un seul host, le même GPU dans les preuves du runner, les caches initialement vides puis non vides, le succès des contrôles sandbox/Wayland/presentation du prototype et sa terminaison vérifiée. Sans croissance, le résultat reste NOT_PROVEN avec growth.status=NOT_OBSERVED. Erreur, limite ou observation interrompue donne INCOMPLETE.

Ce verdict signifie uniquement qu'un worker Mesa disk supplémentaire est apparu dans le même GPU après une baseline sécurisée et a été observé sécurisé. /proc n'expose ni l'identité de l'instance util_queue, ni le contenu des filtres BPF, ni le flag TSYNC. L'attribution à patch0008 reste liée au hash du runtime, à sa provenance et au probe de politique réel. Aucune qualification M1, latence physique ou mesure de performance n'est produite. Les variables PANEFLOW_M1_* sont retirées de cet environnement expérimental.

## Artefacts et validation effectuée

binding.json, baseline.json, control/{armed,release,observed}.json, thread-observations.jsonl, cache-growth.json, les stdout/stderr, les fichiers run/prototype existants et la copie de fixture conservent les preuves. Le cache expérimental reste disponible après le test; aucune purge automatique d'autres caches n'existe.

Validation effectuée: parsing AST des trois fichiers Python et parsing JavaScript des deux modules sans démarrer de serveur. Le parcours natif, la disponibilité des dépendances, le matching du build et la croissance réelle restent à exécuter. Le répertoire contient les hashes des sources de départ et de la proposition.

## Premier essai natif et correction de la gate

Le premier essai avec ANGLE/Vulkan a vérifié les 120 programmes et tous les
threads recensés, mais reste NOT_PROVEN : les workers supplémentaires étaient
nés avant la baseline, lors de la création du contexte WebGL et du premier
shader de préparation. La gate est désormais placée avant toute création de
contexte ou compilation WebGL. Le premier affichage de la page et le census
natif précèdent cette charge. Les critères de naissance, identité, sécurité,
confirmation et croissance ne changent pas.

Le second essai reste NOT_PROVEN : même sans WebGL avant la gate, la composition
initiale crée déjà tous les workers avant la première frame. Le journal montre
une fenêtre entre host_ready et la création du navigateur. La baseline suivante
commence donc dès host_ready, avec le même census de sécurité intégral, avant
l'attente de première frame. La gate WebGL reste fermée jusqu'au census réussi.
La preuve recherchée porte sur la naissance de workers après cette baseline,
sans attribuer chaque naissance à la charge WebGL plutôt qu'au démarrage visuel.


## Preuve bornée du 7 septembre 2026

Le reçu `bench/browser/evidence/tsync-mesa-growth-20260907/cache-growth.json`
porte `PASS_BOUNDED_GROWTH`. Ses 18 fichiers archivés ont été revérifiés par
SHA-256 le 7 septembre. La preuve couvre la naissance de workers après la
baseline, leurs observations de sandbox et 120 shaders avec le runtime
diagnostic `64c291f64dd6505e9a6faa56f12dbc3f02fd99cc0a35811e8bb689d98e392b23`.
Les essais antérieurs ci-dessus restent conservés. Cette preuve ne certifie ni
M1, ni les pixels présentés par GPUI, ni le futur runtime durci.

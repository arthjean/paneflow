# Diagnostic et correction du browser Windows, 11 septembre 2026

Les captures du matin isolaient deux sources de saccades. La première, la
recherche synchrone des binaires d'agents sur le thread principal, est corrigée
et mesurée : les pauses de présentation disparaissent. La seconde, une queue
d'environ 240 ms sur certaines transitions de taille, est maintenant localisée
dans le pipeline de capture de CEF et reste ouverte : cinq variantes ciblées ont
été mesurées, aucune ne la déplace.

## Protocole et validité

Trois répétitions de 20 secondes de repos animé, 20 secondes de scroll et 20
secondes de resize, précédées chacune de 5 secondes de préparation. Les sondes
ciblées utilisent 5 à 8 secondes par phase. Une seule session terminal inactive
accompagne le browser.

Le collecteur réutilise exactement la fixture Linux : 1000 cartes dans une
grille responsive, en-tête fixe et animation. SHA-256 commun vérifié :
`d1db2b507209e238d1c3d757ddd1f0560d51bcf316ec733af220c1cfcacd81a0`. La référence
est [Linux optimized-v5-cadence](../2026-09-07T18-36-38.348Z-optimized-v5-cadence/summary.json).

Windows 11 build 26200, Ryzen 7 7800X3D, RTX 4070 Ti SUPER, pilote 32.0.16.1656,
écran 2560 x 1440 à environ 120 Hz (119 déclaré par WMI), échelle 100 %, fenêtre
1920 x 1080. CEF utilise une cadence de base de 60 Hz. Le scroll natif injecte 15
événements par seconde, le drag du séparateur 60 positions par seconde avec une
amplitude de 180 pixels logiques et une période de 2 secondes.

La référence Linux utilise un écran 144 Hz et des gestes manuels. Toute
comparaison avec elle est descriptive et ne constitue pas un verdict de
régression entre OS. PresentMon mesure la présentation de la fenêtre, pas la
latence physique entre entrée et photon.

Les six captures retenues ont terminé avec zéro événement de journal perdu et
zéro événement PresentMon non corrélé. Le statut reste
`OBSERVED_NOT_BUDGET_CERTIFIED` : ces captures courtes ne certifient aucun
budget NFR.

Le binaire `avant` et le binaire `après` diffèrent aussi par les sondes ajoutées
pendant cette campagne. Les sondes du chemin de peinture sont désactivées par
défaut et n'étaient pas actives pendant les trois répétitions finales ; les
événements d'étape de resize du host, eux, sont actifs sous
`PANEFLOW_BROWSER_BENCH` et ajoutent trois messages de contrôle par resize à la
campagne `après` uniquement. Ce surcoût joue contre l'amélioration mesurée, pas
en sa faveur.

## Bilan avant/après

Trois répétitions de chaque côté, mêmes durées, même fixture, même machine, même
configuration d'écran.

| Mesure | Avant r1 | r2 | r3 | Après r1 | r2 | r3 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Pauses de présentation > 50 ms | 33 | 33 | 33 | **0** | **0** | **0** |
| Images reçues avec plus de 50 ms de retard | 46 | 46 | 47 | **0** | **0** | **0** |
| Intervalle entre images, scroll, max (ms) | 150,615 | 155,516 | 142,235 | **32,888** | **31,916** | **32,939** |
| Intervalle entre images, repos, max (ms) | 141,837 | 143,765 | 145,894 | **33,074** | **32,888** | **32,577** |
| Intervalle d'affichage PresentMon, max (ms) | 150,008 | 150,000 | 141,672 | **8,700** | **10,276** | **8,456** |
| Host prêt vers réception app, p99 (ms) | 121,636 | 117,951 | 118,331 | **1,960** | **1,704** | **1,718** |
| Images en 20 s, repos | 1139 | 1135 | 1141 | **1200** | **1200** | **1200** |
| Images en 20 s, scroll | 1140 | 1141 | 1143 | **1200** | **1200** | **1200** |
| Resize vers bonne taille, p50 (ms) | 27,799 | 28,605 | 27,513 | 27,390 | 28,333 | 28,916 |
| Resize vers bonne taille, p95 (ms) | 245,939 | 253,469 | 246,282 | 241,368 | 243,618 | 244,712 |
| Resize vers bonne taille, p99 (ms) | 254,307 | 264,415 | 286,427 | 250,586 | 250,561 | 256,223 |
| Transitions de resize > 100 ms | 54 | 52 | 52 | 52 | 51 | 51 |

Le scroll et le repos perdent toutes leurs pauses et atteignent exactement la
cadence de capture de 60 Hz sur 20 secondes. Le resize est inchangé : les
écarts de p95 et p99 restent dans la dispersion entre répétitions, aucune
amélioration n'est revendiquée sur cet axe, et aucune régression n'est visible.

## 1. Recherche des binaires d'agents : corrigé

`InstalledBinaryCache::refresh` exécutait les seize appels `which::which` sur le
thread principal quand le cache expirait, soit toutes les 2 secondes, pour une
durée médiane de 122,691 ms. Les 11 pauses de présentation de la sonde du matin
coïncidaient exactement avec les 11 recherches.

Le cache sert désormais son instantané immédiatement et délègue le
rafraîchissement à un seul thread nommé `paneflow-agent-scan`, réveillé à la
demande par un canal de capacité 1. Une recherche déjà en cours n'en déclenche
pas une seconde. Le tout premier accès, avant toute recherche, reste synchrone :
la visibilité des agents au premier rendu est donc identique à l'ancien
comportement, et Linux comme macOS empruntent exactement le même chemin. Si le
thread ne peut pas démarrer, l'appel retombe sur une recherche synchrone.

Après correction, les trois répétitions mesurent 32 à 33 recherches sur 70
secondes, toutes sur `ThreadId(113):paneflow-agent-scan`, jamais sur
`ThreadId(1):main`, pour une médiane de 126,6 à 130,9 ms. Aucune de ces
recherches ne chevauche une pause de présentation, et il n'existe plus aucune
pause de présentation supérieure à 50 ms.

Cinq tests unitaires couvrent la décision du cache : premier accès bloquant,
instantané frais servi sans recherche, instantané périmé servi immédiatement
avec recherche en arrière-plan, recherche déjà en cours non redemandée,
remplacement de l'instantané.

## 2. Retard de resize : localisé, non corrigé

La sonde du chemin de peinture, activée par `PANEFLOW_BROWSER_PAINT_PROBE=1`,
enregistre chaque rappel `on_accelerated_paint` tant qu'un resize n'est pas
stabilisé, avec la taille codée, la taille attendue, l'état des pools, l'issue,
l'horodatage de capture et le compteur de capture de CEF.

Sur les 75 transitions de `capture-probe-r1` :

| Étape | p50 (ms) | p95 (ms) |
| --- | ---: | ---: |
| Envoi application vers réception host | 0,158 | 0,192 |
| Réception host vers création/annonce du pool | 0,392 | 0,793 |
| Réception host vers pool marqué prêt par l'acquittement | 1,555 | 1,857 |
| Réception host vers premier rappel à la bonne taille | 28,263 | 244,264 |
| Réception host vers publication | 28,295 | 244,277 |

Trois conclusions tiennent sur ces mesures.

Le pool n'est jamais le facteur limitant : il est créé en 0,4 ms et acquitté
prêt en 1,6 ms, soit deux ordres de grandeur sous le retard observé.

Le host publie la première image à la bonne taille dès qu'elle arrive : sur les
75 transitions, le compteur de rappels à la bonne taille vaut exactement 1, et
son issue est toujours `published`. Aucune image à la bonne taille n'est rejetée
pour absence de pool, absence de tampon libre ou limite d'images en vol. Les
seules issues observées sont `stale_geometry` (315) et `published` (74).

Le retard est une interruption de livraison à l'intérieur de CEF. Pendant la
transition, CEF continue de livrer des images à l'ancienne taille, puis se tait
pendant 120 à 210 ms, puis vide sa file d'un coup : dix à douze rappels arrivent
en moins de 0,5 ms. Les compteurs de capture sont consécutifs et leurs
horodatages de capture sont espacés de 8,3 ms, donc ces images ont bien été
capturées à 120 Hz pendant le silence et livrées avec 100 à 200 ms de retard.
Suit un trou de 110 à 135 ms sans aucune capture, puis la première image à la
nouvelle taille. Le diagnostic compte 15 de ces interruptions dans une fenêtre
de 6 secondes, médiane 184,212 ms, exactement autant que de transitions au-delà
de 100 ms.

Le thread UI du host est hors de cause : aucune section instrumentée
(`present`, `paint`, `create_pool`, `retire_pool`) n'a dépassé 5 ms, et la pompe
de resize n'a jamais manqué son tick de 16 ms de plus de 25 ms pendant toute une
capture. Le thread principal de l'application est hors de cause également : la
queue subsiste à l'identique après la correction 1, et l'acquittement de pool
revient en 1,5 ms.

### Hypothèses testées et écartées

Chaque variante a été mesurée séparément, sur la même fixture et la même
machine, avec les sondes actives. Les données sont dans
[resize-experiments.json](resize-experiments.json).

| Variante | p50 (ms) | p95 (ms) | Transitions > 100 ms |
| --- | ---: | ---: | ---: |
| Référence, glissement 60 Hz | 28,478 | 244,482 | 22 % |
| `was_hidden` réservé aux changements de visibilité, comme Linux | 27,844 | 248,274 | 22 % |
| Glissement à 15 Hz | 28,983 | 248,919 | 28 % |
| Glissement à 5 Hz | 35,487 | 267,027 | 50 % |
| Pompe de resize sans `invalidate` | 28,828 | 275,679 | 22 % |
| Sans accélération de cadence à 120 Hz | 30,341 | 237,874 | 23 % |

L'appel Windows à `was_hidden` à chaque présentation, qui était la piste
proposée, ne porte pas ce retard : le réserver aux changements de visibilité ne
change rien de mesurable. Espacer les changements de taille aggrave la
proportion de transitions lentes au lieu de la réduire, ce qui écarte aussi une
correction par étranglement du débit de resize. Rapportée au temps, la queue se
comporte comme une interruption d'environ 200 ms toutes les 350 à 400 ms de
glissement continu, quelle que soit la cadence des changements de taille.

La piste restante, non testée ici, est la réallocation de la surface de capture
côté Chromium lors d'un changement de résolution en rendu hors écran accéléré.
La départager demande soit une trace Chromium interne, soit une comparaison avec
une page légère, ce qui sortirait de la fixture pinée de ce protocole.

## Reproduction et preuves

Depuis la racine du dépôt, avec le binaire release compilé, un bundle CEF
Windows signé et `target/PresentMon-2.5.1-x64.exe` :

```powershell
bun scripts/browser-qualification/windows-dock-benchmark.mjs target/browser-lag-fix-staging-20260911 target/browser-lag-new-capture 3 20
bun scripts/browser-qualification/windows-dock-diagnosis.mjs target/browser-lag-new-capture
```

Pour rejouer la sonde du chemin de peinture, ajouter
`PANEFLOW_BROWSER_PAINT_PROBE=1` et raccourcir les phases. Pour rejouer les
variantes, ajouter `PANEFLOW_BROWSER_RESIZE_REFRESH=0` ou
`PANEFLOW_DOCK_DRAG_HZ=15`. Le dossier de sortie doit être nouveau. Garder la
fenêtre de benchmark au premier plan sans toucher aux entrées pendant la
capture.

Le collecteur utilise `target/release/paneflow.exe` et inscrit les empreintes
des exécutables et de la fixture dans chaque `metadata.json`. Les empreintes des
binaires avant et après sont dans [environment.json](environment.json).

Archivé ici : `baseline-r1` à `baseline-r3` et `probe-r1` pour l'état initial,
`final-r1` à `final-r3` pour l'état corrigé, `capture-probe-r1` pour la sonde du
chemin de peinture, les diagnostics
[baseline-diagnosis.json](baseline-diagnosis.json),
[probe-diagnosis.json](probe-diagnosis.json),
[final-diagnosis.json](final-diagnosis.json),
[capture-probe-diagnosis.json](capture-probe-diagnosis.json), et la matrice
d'expériences [resize-experiments.json](resize-experiments.json).

Les traces brutes locales restent dans `target/browser-lag-baseline2-20260911`,
`target/browser-lag-probe-20260911`, `target/browser-lag-fix1-20260911`,
`target/browser-lag-final-20260911`, `target/browser-lag-captureprobe-20260911`,
`target/browser-lag-washidden-20260911`, `target/browser-lag-drag15-20260911`,
`target/browser-lag-drag5-20260911`, `target/browser-lag-norefresh-20260911` et
`target/browser-lag-noboost-20260911` : `events.jsonl`, `application.jsonl`,
`inputs.jsonl`, `presentmon.csv`. Les binaires de chaque étape sont préservés
dans `target/browser-lag-fix-20260911/artifacts`.

Le champ `capture_delivery_stalls` vaut 0 dans
[final-diagnosis.json](final-diagnosis.json) parce que la sonde du chemin de
peinture était désactivée pendant ces captures, pas parce que les interruptions
auraient disparu.

## Validation effectuée

`cargo fmt --check` propre, `cargo clippy --workspace --all-targets --locked -D
warnings` propre, `cargo clippy -p paneflow-browser-host --features cef-runtime
--all-targets --target x86_64-pc-windows-msvc -D warnings` propre, 20 tests
unitaires de `agent_launcher` réussis dont 5 nouveaux, 4 tests des collecteurs
réussis. Trois répétitions longues, une sonde de peinture et cinq sondes
d'expérience réussies, toutes valides. Linux et macOS n'ont pas été réexécutés :
la correction du cache est commune aux trois plateformes et ne dépend d'aucune
API spécifique à Windows, mais elle n'a été mesurée que sur Windows. Aucun
commit ni push effectué.

## Limites

Ces captures observent une machine, un écran, un pilote et une fixture. Elles ne
certifient aucun budget. La correction 1 supprime les pauses mesurées dans ce
cadre ; elle laisse une recherche synchrone au tout premier accès et conserve
une fenêtre de péremption de 2 secondes, donc la visibilité d'un agent installé
pendant la session peut mettre jusqu'à environ 2,2 secondes à apparaître, contre
2 secondes avant. Le retard de resize n'est pas corrigé et aucun gain n'est
revendiqué sur cet axe.

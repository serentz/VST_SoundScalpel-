# SoundScalpel — guide utilisateur

SoundScalpel est un effet VST3 pour Windows, pensé pour Ableton Live 12, qui nettoie un son au scalpel : il retire les sifflements et résonances agressives, et peut redonner leurs aigus aux stems étouffés par une séparation.

Ce guide couvre la version actuelle du plugin.

---

## Sommaire

1. [Le principe : soustraire, pas égaliser](#1-le-principe--soustraire-pas-égaliser)
2. [Installation](#2-installation)
3. [Mise en place dans Live](#3-mise-en-place-dans-live)
4. [L'interface](#4-linterface)
5. [Résonances et sifflements](#5-résonances-et-sifflements)
6. [Bruit large bande (souffle)](#6-bruit-large-bande-souffle)
7. [Restauration de stem (aigus perdus)](#7-restauration-de-stem-aigus-perdus)
8. [Écouter, comparer, régler](#8-écouter-comparer-régler)
9. [Enregistrer le résultat](#9-enregistrer-le-résultat)
10. [SoundScalpel et compresseur](#10-soundscalpel-et-compresseur)
11. [Dépannage](#11-dépannage)
12. [Référence des réglages](#12-référence-des-réglages)
13. [Limites actuelles](#13-limites-actuelles)
14. [Licence](#14-licence)

---

## 1. Le principe : soustraire, pas égaliser

Un égaliseur, même dynamique, creuse une **bande de fréquences** : il enlève tout ce qui s'y trouve, le défaut comme le son utile, et sa cloche reste en place quand le défaut change de hauteur.

SoundScalpel procède autrement. Il **recrée l'onde du défaut** (sa fréquence, son amplitude et sa phase), la suit quand elle glisse, puis l'**ajoute en opposition de phase** au son d'origine. Le défaut s'annule ; le reste du son, même dans la même zone de fréquences, n'est pas touché. Et comme le défaut est annulé plutôt qu'atténué, un compresseur placé ensuite n'a rien à faire remonter.

Le même circuit sert aussi dans l'autre sens : pour la restauration de stem, SoundScalpel calcule ce qui **manque** au son et l'ajoute.

---

## 2. Installation

SoundScalpel se construit à partir des sources. Il faut [Rust](https://rustup.rs) et [Git](https://git-scm.com) installés.

Dans PowerShell :

```powershell
git clone https://github.com/serentz/VST_SoundScalpel- "I:\POC\VST_SoundScalpel-"
```

```powershell
Set-Location "I:\POC\VST_SoundScalpel-"
```

```powershell
cargo xtask bundle soundscalpel --release
```

**Ferme Live**, puis copie le plugin dans le dossier VST3 de Windows (PowerShell en administrateur si Windows refuse) :

```powershell
Copy-Item -Recurse -Force "target\bundled\soundscalpel.vst3" "C:\Program Files\Common Files\VST3\"
```

Rouvre Live : SoundScalpel apparaît dans les plugins VST3 (éditeur « Serentz »). Si ce n'est pas le cas, lance une nouvelle analyse des plugins dans les préférences de Live.

**Mettre à jour** : `git pull origin main`, puis les deux dernières commandes ci-dessus. Le premier `cargo xtask bundle` prend quelques minutes ; les suivants sont rapides.

---

## 3. Mise en place dans Live

### Le montage recommandé : un rack à deux chaînes

1. Sur la piste à traiter, crée un **Audio Effect Rack**.
2. Crée **deux chaînes** :
   - une chaîne **vide** (le son original) ;
   - une chaîne contenant **SoundScalpel**, en mode de sortie **« Parasite inversé »** (le réglage par défaut).
3. Les deux chaînes restent à **0,0 dB** et **au centre**, et **aucun effet ne doit suivre SoundScalpel** dans sa chaîne.

Le rack additionne les deux chaînes : le son original plus le défaut inversé donnent le son nettoyé.

### Vérifier l'alignement : le test d'annulation

Avant le premier usage, clique sur **« Test d'annulation »** dans l'interface du plugin. La piste doit devenir **muette** : c'est la preuve que Live aligne bien les deux chaînes. Clique à nouveau pour revenir au fonctionnement normal.

> ⚠️ **Ne désactive pas SoundScalpel avec son bouton d'activation Live** quand il est dans le rack. Live laisse alors passer le son tel quel, qui s'ajoute à la chaîne originale : le son est **doublé** (+6 dB). Pour couper le traitement, **coupe la chaîne** SoundScalpel avec le bouton de la chaîne.

### Sans rack : le mode « Signal nettoyé »

Si tu préfères une insertion directe, mets SoundScalpel directement sur la piste et choisis **« Signal nettoyé »** : le plugin sort lui-même le son corrigé. Le résultat est identique ; tu perds seulement la possibilité de couper la correction depuis la chaîne.

### Latence

SoundScalpel travaille sur des fenêtres d'environ 43 ms : il déclare une latence de 2048 échantillons (4096 au-delà de 60 kHz), que Live compense automatiquement. C'est normal et sans effet sur le calage de tes pistes.

---

## 4. L'interface

### L'analyseur (au centre)

De 20 Hz à 20 kHz, en dBFS :

| Couleur | Signification |
|---|---|
| Gris-violet (rempli) | Le spectre d'entrée |
| **Orange** | Ce qui est retiré, ou ajouté par la restauration de stem |
| Ligne claire | La sortie corrigée |
| Trait orange avec fréquence | Une résonance en cours de soustraction ; plus il est opaque, plus elle est retirée |
| Zones assombries | Hors de la plage de fréquences traitée par le moteur de résonances |

Quand Live n'envoie pas de son, l'analyseur affiche « En attente de signal : lance la lecture ».

> Dans le grave, une résonance unique peut dessiner une large forme orange : chaque case d'analyse y fait une vingtaine de hertz et l'échelle est logarithmique. Le plugin ne retire pourtant qu'une seule onde.

### La colonne de droite

De haut en bas :

- **Intensité** : dose de la correction appliquée (100 % = toute la correction).
- **ÉCOUTER LA CORRECTION** : fait entendre uniquement la correction — ce qui est retiré (résonances, bruit) ou ce qui est ajouté (restauration de stem). Le bouton et un bandeau sur l'analyseur passent au rouge quand l'écoute est active.
- **Sortie** : « Parasite inversé » (rack) ou « Signal nettoyé » (insertion directe).
- **Test d'annulation** : voir la [section 3](#vérifier-lalignement--le-test-dannulation).
- **Latence déclarée**.
- Les réglages des trois moteurs, sous trois onglets.

En bas de la fenêtre, la liste des **résonances retirées** indique leur fréquence et leur niveau.

L'écoute et le test d'annulation **ne sont pas enregistrés avec le projet, et ne peuvent pas être automatisés** : un projet se rouvre toujours en écoute normale, pour éviter d'enregistrer la correction seule par accident. Tous les autres réglages (curseurs et choix listés plus bas) sont au contraire des paramètres ordinaires, automatisables et sauvegardés avec le projet — voir la [référence des réglages](#12-référence-des-réglages).

---

## 5. Résonances et sifflements

### Quand l'utiliser

Pour les sons qui « arrachent les oreilles » : sifflements, résonances de filtre trop pointues, larsen, ronflements à fréquence fixe, résonances d'un synthé ou d'un sample. Le moteur cherche des **pics étroits, persistants et stables en niveau**, ce qui les distingue des notes de musique, qui décroissent ou changent.

### Comment il décide

Un pic devient une résonance à retirer seulement s'il remplit trois conditions :

1. il dépasse son voisinage d'au moins **Sensibilité** dB ;
2. il est présent depuis au moins **Persistance** ms ;
3. son niveau varie moins que **Variation tolérée** et reste dans une fourchette d'environ 4 dB.

Le retrait commence et s'arrête en fondu, sans clic. Pendant une attaque de note qui brouille la mesure, le moteur prolonge brièvement la résonance précédente au lieu de suivre la perturbation.

### Réglages

| Réglage | Effet | Conseil |
|---|---|---|
| Résonances (Actif / Inactif) | Active le moteur | — |
| Sensibilité | Écart minimal d'un pic au-dessus de son voisinage | Plus bas = plus de pics retirés ; 15 dB par défaut |
| Persistance avant retrait | Durée de présence exigée | Plus haut = plus prudent avec les notes tenues |
| Variation de niveau tolérée | Pente de niveau admise | Plus bas = épargne mieux les notes qui décroissent |
| Fréquence basse / haute | Plage traitée | Monte la fréquence basse pour protéger basse et kick |

### Conseils de réglage

Compare avant et après en coupant puis en réactivant la chaîne SoundScalpel du rack. Le son doit devenir moins agressif sans perdre son mordant. S'il devient terne, baisse « Intensité » ou remonte « Sensibilité » vers 15 dB.

Sur de la musique électronique, les sons vraiment agressifs se trouvent surtout entre 2 et 6 kHz. Si le moteur s'attaque à la basse ou au kick (une résonance vers 100 Hz par exemple), monte **Fréquence basse** vers 200–300 Hz.

---

## 6. Bruit large bande (souffle)

### Le principe, et sa limite assumée

Un bruit blanc ou un souffle est aléatoire : contrairement à un sifflement, sa forme d'onde ne se déduit ni de son passé ni du reste du signal, donc il n'y a rien à synthétiser puis annuler. Ce moteur **atténue** le bruit au lieu de l'annuler : il estime un plancher de bruit par case de fréquence et réduit ce qui lui ressemble. Un résidu reste toujours sous les notes — c'est inhérent à la méthode, pas un réglage à corriger.

### Donner un profil au moteur

Deux façons d'obtenir le plancher de bruit, par ordre de préférence :

1. **Apprendre le bruit** : lis un passage qui ne contient que le bruit (avant l'entrée d'un instrument, par exemple), clique sur **APPRENDRE LE BRUIT**, laisse jouer 1 à 2 secondes, puis clique à nouveau pour arrêter — le profil se **gèle** automatiquement à l'arrêt. **Remettre à zéro** efface ce profil.
2. **Estimation automatique** : à défaut de passage de bruit seul, le moteur suit un plancher conservateur en continu. Désactive-la si elle confond les notes tenues d'un instrument avec du bruit.

La ligne d'état indique quel profil est actif : profil appris et gelé, estimation automatique, ou aucun profil.

### Réglages

| Réglage | Effet | Conseil |
|---|---|---|
| Bruit (Actif / Inactif) | Active le moteur | Désactivé par défaut |
| Apprendre le bruit | Capture le profil pendant qu'il est actif | Sur un passage de bruit seul uniquement |
| Estimation automatique | Plancher conservateur sans profil appris | Désactive-la sur un instrument très tenu |
| Réduction maximale | Plafond d'atténuation | 12 dB par défaut ; prudent sur un signal dense |
| Lissage | Stabilité du gain dans le temps | Plus haut = moins de bruit musical, moins réactif |

### Conseils de réglage

Sur un signal dense (plusieurs voix ou harmoniques qui se chevauchent en permanence), toute atténuation du bruit mord un peu sur le signal lui-même : c'est le cas que le cahier des charges déclare non séparable sans altération. Monte « Réduction maximale » progressivement à l'oreille, en écoutant la correction (section 8), plutôt que de viser un chiffre. Sur une source bien séparée du bruit (une voix isolée, un instrument seul), le gain est net et sans compromis.

---

## 7. Restauration de stem (aigus perdus)

### Le problème

Un stem issu d'une séparation (celle de Live, Demucs, RoFormer…) sonne souvent **étouffé**, avec un effet de tunnel : dans les aigus, où les sources se chevauchent, le séparateur attribue les zones douteuses à un autre stem. Ces aigus ne sont pas détruits : ils sont **restés dans le mix d'origine**.

Choisis le mode dans la section **« Restauration de stem (aigus perdus) »** :

| Mode | Quand | Résultat |
|---|---|---|
| **Désactivée** | Le stem n'est pas étouffé | Aucun changement |
| **A : avec le mix** | Tu as le fichier du mix d'origine | Les vrais aigus, repris dans le mix |
| **B : sans référence** | Tu n'as que le stem | Aigus remontés, et recréés si besoin |

### Mode A : avec le mix d'origine

1. Place le **mix d'origine** sur une piste du projet, calée exactement au même point de départ que le stem. Elle peut être coupée dans le mix final, à condition qu'elle joue.
2. Sur la piste du stem, mets SoundScalpel dans le rack habituel et choisis **« A : avec le mix »**.
3. Déplie le panneau **Sidechain** du plugin dans Live, active-le et choisis la piste du **mix d'origine** comme source.
4. Lis la ligne d'état dans l'interface :

| Message | Signification |
|---|---|
| « Aucun signal sur la sidechain… » (orange) | La sidechain n'est pas réglée, ou la piste du mix ne joue pas |
| « Mix reçu, mesure de l'alignement en cours… » | Attends quelques secondes de lecture |
| « Mix reçu, aligné sur le stem. » (vert) | Tout est prêt |
| « Stem en retard / en avance de N éch. sur le mix » (rouge) | Décale une des deux pistes de N échantillons |

Réglages :

- **Début de restauration** (2,5 kHz par défaut) : fréquence à partir de laquelle les aigus sont restaurés.
- **Récupération** : part des aigus manquants reprise dans le mix. En contrepartie, un peu des autres sources du mix (hi-hats, cymbales) revient dans le stem ; baisse ce réglage si tu en entends trop.

### Mode B : sans référence

Le plugin compare le niveau des aigus à celui de la bande juste en dessous. Là où les aigus s'effondrent par rapport à la pente attendue, il les remonte dynamiquement ; là où ils ont disparu, il peut en recréer.

- **Début de restauration** : comme en mode A.
- **Pente cible** (−4,5 dB/octave par défaut) : décroissance naturelle attendue des aigus. Plus proche de 0 = son plus brillant.
- **Remontée maximale** (12 dB) : limite de la remontée.
- **Aigus recréés** (0 % par défaut) : aigus fabriqués à partir du médium là où il n'y a plus rien à remonter. Ce sont des aigus **inventés** : dose-les à l'oreille, par petites touches, pour éviter un son métallique.

Le mode B est un choix à part entière quand le mix d'origine n'est pas disponible ; le mode A reste plus fidèle quand tu l'as.

---

## 8. Écouter, comparer, régler

- **Avant / après** : coupe puis réactive la chaîne SoundScalpel du rack (ou utilise le bouton d'activation de Live en mode « Signal nettoyé » uniquement).
- **La correction seule** : clique sur **ÉCOUTER LA CORRECTION**. Dans le rack, le plugin compense lui-même la chaîne originale : tu entends uniquement la correction, sans avoir à mettre la chaîne en solo.
  - Pour les résonances, tu ne dois entendre que des sifflements ou des sons agressifs. Si tu reconnais des notes de ta musique, le réglage est trop agressif.
  - Pour la restauration de stem, tu entends les aigus ajoutés.
  - Pour le bruit, tu entends uniquement ce qui est retiré : si tu y reconnais le signal lui-même, baisse « Réduction maximale ».

---

## 9. Enregistrer le résultat

### Réenregistrement sur une piste

1. Règle le traitement et **quitte l'écoute du retiré**.
2. Crée une piste audio de destination. Dans **Audio From**, choisis la piste source, puis **Post FX** (ou **Post Mixer** si tu veux ses réglages de volume et de panoramique).
3. Arme la piste de destination, mets son monitoring sur **Off** pour éviter une écoute doublée, puis enregistre.
4. Vérifie le résultat avant de désactiver la piste source.

Évite **Resampling** si tu ne veux capturer que cette piste : il enregistre toute la sortie principale.

### Sans temps réel

**Bounce to New Track**, ou **Freeze** puis **Flatten**, donnent le même résultat qu'une lecture.

---

## 10. SoundScalpel et compresseur

Un compresseur remonte les sons faibles, donc tout ce qui reste d'un défaut. L'annulation de SoundScalpel laisse peu de résidu, mais la position compte quand même :

- **SoundScalpel avant le compresseur** : le cas habituel ; le compresseur travaille sur un son déjà nettoyé.
- **SoundScalpel après le compresseur** : utile si le compresseur fait lui-même ressortir le défaut ; la détection se fait alors sur le son tel que tu l'entends.

Compare les deux à l'oreille sur ton morceau.

---

## 11. Dépannage

| Symptôme | Cause probable | Solution |
|---|---|---|
| Le son est plus fort, comme doublé | SoundScalpel désactivé avec le bouton Live dans le rack | Réactive-le ; coupe plutôt la chaîne |
| Le test d'annulation laisse un son filtré, « phasé » | Chaînes pas à 0 dB ou pas au centre, ou effet après SoundScalpel | Vérifie les chaînes du rack |
| Le son devient terne | Correction trop forte | Baisse Intensité, remonte Sensibilité vers 15 dB |
| La basse ou le kick perdent de la matière | Le moteur traite le grave | Monte Fréquence basse à 200–300 Hz |
| L'analyseur affiche « En attente de signal » | Pas de lecture, ou piste muette | Lance la lecture |
| Mode A : « Aucun signal sur la sidechain » | Sidechain non réglée, ou piste du mix muette | Règle la source Sidechain ; fais jouer la piste du mix |
| Mode A : stem décalé de N échantillons | Pistes stem et mix mal calées | Recale l'une des deux pistes |
| La copie du plugin échoue (« fichier occupé ») | Live utilise le plugin | Ferme Live avant de copier |
| Le bruit réduit assombrit aussi le signal | Signal dense, réduction trop forte | Baisse « Réduction maximale » ; vérifie le profil (appris vs automatique) |
| Comportement étrange après une mise à jour | Instance créée avec une ancienne version | Remplace-la par une nouvelle instance |

---

## 12. Référence des réglages

| Réglage | Plage | Défaut | Automatisable |
|---|---|---|---|
| Intensité | 0 – 100 % | 100 % | Oui |
| Mode de sortie | Parasite inversé / Signal nettoyé | Parasite inversé | Oui |
| Résonances | Actif / Inactif | Actif | Oui |
| Sensibilité | 6 – 40 dB | 15 dB | Oui |
| Persistance | 50 – 2000 ms | 300 ms | Oui |
| Variation tolérée | 0,5 – 20 dB/s | 4 dB/s | Oui |
| Fréquence basse | 20 Hz – 2 kHz | 40 Hz | Oui |
| Fréquence haute | 1 – 20 kHz | 18 kHz | Oui |
| Restauration de stem | Désactivée / A / B | Désactivée | Oui |
| Début de restauration | 500 Hz – 10 kHz | 2,5 kHz | Oui |
| Récupération (A) | 0 – 100 % | 100 % | Oui |
| Pente cible (B) | −12 – 0 dB/octave | −4,5 dB/octave | Oui |
| Remontée maximale (B) | 0 – 24 dB | 12 dB | Oui |
| Aigus recréés (B) | 0 – 100 % | 0 % | Oui |
| Bruit | Actif / Inactif | Inactif | Oui |
| Apprendre le bruit | Actif / Inactif | Inactif | Oui |
| Estimation automatique (bruit) | Actif / Inactif | Actif | Oui |
| Réduction maximale (bruit) | 0 – 24 dB | 12 dB | Oui |
| Lissage (bruit) | 20 – 500 ms | 150 ms | Oui |
| Remettre à zéro le profil de bruit | — | — | **Non** |
| Écouter ce qui est retiré | — | Écoute normale | **Non** |
| Test d'annulation | — | Désactivé | **Non** |

Tous ces réglages sont de vrais paramètres VST3 : automatisables dans Live, et sauvegardés avec le projet et les presets. Les deux seules exceptions sont l'écoute du retiré et le test d'annulation, volontairement exclus pour qu'un projet se rouvre toujours en écoute normale (voir [section 4](#4-linterface)).

### Automatiser un réglage dans Live

1. Active le mode automation de l'Arrangement (le bouton en haut à droite, ou `Ctrl+A`).
2. Dans le sélecteur de dispositif/paramètre de la voie d'automation de la piste, choisis **SoundScalpel**, puis le réglage voulu (par exemple **Récupération** ou **Intensité**) dans la liste.
3. Dessine la courbe. Les changements s'appliquent par pas d'environ 10 ms (le rythme interne de l'analyse) : pas de clic à craindre.

---

## 13. Limites actuelles

- **Bruit large bande** : le moteur atténue, il n'annule pas — un bruit aléatoire ne peut pas être recréé puis soustrait comme un sifflement. Sur un signal musical dense, la réduction mord nécessairement un peu sur le signal ; le réglage par défaut reste volontairement prudent.
- **Stéréo** : chaque canal est analysé séparément ; la détection liée entre gauche et droite est prévue.
- **Sons très tenus et stables** (drones, pads sans évolution, sons de cloche) : le moteur de résonances peut les prendre pour des défauts. Écoute ce qui est retiré et ajuste Persistance, Variation tolérée ou la plage de fréquences.
- **Restauration de stem** : aucun des deux modes ne rend exactement le stem idéal. Le mode A ramène un peu des autres sources du mix ; le mode B n'invente des aigus que s'ils sont demandés.
- Plateforme : Windows x64, VST3, testé avec Ableton Live 12.

---

## 14. Licence

SoundScalpel est publié sous **GPL-3.0-or-later** (texte complet dans [LICENSE](../LICENSE)). Ce choix n'est pas arbitraire : `vst3-sys`, la bibliothèque qui relie nih-plug au SDK VST3 de Steinberg, est elle-même sous GPLv3 — un binaire compilé qui l'embarque est une œuvre dérivée, et sa distribution doit respecter la GPLv3. Le détail des licences de chaque dépendance est dans [docs/THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).

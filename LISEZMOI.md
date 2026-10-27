# Hermes Map

Explorateur visuel de tes dossiers de notes : une carte mentale (mind map) des dossiers, des fichiers et des liens entre eux, avec un panneau de lecture à droite. Fait pour accompagner l'agent **Hermes** (commande `hermes`), mais fonctionne aussi sans lui, sur n'importe quel dossier de notes (Markdown, coffre Obsidian, iCloud Drive…).

## Installer et lancer

**Sur Mac (recommandé) : construire l'app.** Il faut les outils de développement d'Apple (ils fournissent aussi Python 3) :

```
xcode-select --install
git clone https://github.com/AnthonyDpz/HermesMap.git
cd HermesMap
./macos/build_app.sh --install
```

**Hermes Map.app** est alors dans ton dossier **Applications** personnel (`~/Applications`). Lance-la avec Spotlight (⌘ + Espace, « Hermes Map ») ou glisse-la dans le Dock. La carte s'ouvre dans ton navigateur, sans fenêtre de Terminal.

**Sans construire l'app** (Mac, Linux) : `python3 hermes_map.py`, ou double-clic sur `Lancer Hermes Map.command` sur Mac. Seul Python 3 est nécessaire, sans aucune dépendance.

**Arrêter** : bouton **⏻** en haut à droite, ou rien du tout. L'app s'arrête toute seule deux minutes après la fermeture du dernier onglet, mais jamais pendant que Hermes travaille.

En cas de problème, le journal de l'app est dans `~/Library/Logs/Hermes Map.log`.

## Premier lancement

Un écran d'accueil pose trois questions :

1. **Hermes** : l'app cherche la commande `hermes` et le dossier `~/.hermes` (avec ses profils). Si Hermes n'est pas installé, la carte fonctionne quand même ; les fonctions Hermes apparaîtront quand il le sera.
2. **Où sont tes notes ?** L'app propose ce qu'elle trouve : iCloud Drive, tes coffres Obsidian, `~/.hermes`, Documents. Tu peux aussi **choisir un autre dossier** ou coller un chemin. Ensuite, **coche les dossiers qui contiennent tes notes** : ce sont eux qui forment la carte (le côté « notes »). Aucun préfixe n'est nécessaire. La case « Afficher aussi les dossiers créés plus tard » décide si un nouveau dossier apparaît tout seul ou reste masqué jusqu'à ce que tu le coches.
3. **Voulez-vous centraliser les fichiers et binaires dans un dossier d'archivage ?**
   - **Non** : tout reste où c'est.
   - **Oui** : choisis le **dossier de stockage miroir**, un dossier existant ou un nouveau. Les notes restent dans leurs dossiers, et les fichiers (images, vidéos, PDF, documents…) vont dans le dossier de stockage, au **même chemin** : la note `Perso/voyages/japon.md` et ses photos dans `Stockage/perso/voyages/`. La carte montre les deux ensemble, et le reclassement (à la main ou par Hermes) suit cette règle. Rien n'est déplacé au moment du choix. Un nouveau dossier de stockage est créé avec un `README.md`, un catalogue `CATALOG.jsonl` et un journal `LOG.md` ; dans un dossier existant, rien n'est ajouté.

Tout se modifie ensuite avec le bouton **⚙** (Réglages). Dans les réglages avancés, un **préfixe masqué** (facultatif) permet d'afficher `N_Pro` sous le nom « Pro ». Les réglages sont enregistrés dans `~/Library/Application Support/hermes-map/config.json` (Mac) ou `~/.config/hermes-map/config.json`.

**Accès aux dossiers protégés (Mac).** Pour lire iCloud Drive ou d'autres dossiers protégés, macOS demande d'autoriser l'app. L'écran l'explique et le bouton **Ouvrir les Réglages** ouvre la bonne page : glisse Hermes Map dans **Réglages Système → Confidentialité et sécurité → Accès complet au disque**, active-la, puis relance l'app. C'est à faire une seule fois (à refaire si tu reconstruis l'app).

## Ce que l'app affiche

- **Racine** au centre, puis chaque dossier de premier niveau (une **zone**) avec sa couleur. Dans la barre en bas à gauche, clic sur le nom = aller à la zone, clic sur l'œil = la masquer.
- **Tags**, dans la barre latérale (avec recherche), en trois groupes :
  - **Zones** : un par dossier de premier niveau.
  - **Tags de tes notes (◆ roses)** : écrits dans l'en-tête des fichiers Markdown (`tags: [pro, client]`) ou dans le texte (`#idee`).
  - **Tags automatiques (◆ bleus)** : mots qui reviennent dans les noms de fichiers et de dossiers de **plusieurs branches**. Ils relient ce qui est éparpillé.

  Un clic sur un tag cadre la carte sur ses éléments et ouvre à droite la liste **complète**, avec les filtres. Le bouton **Tags** masque ou affiche les ◆ sur la carte.
- **Traits pleins** : arborescence dossier → fichier. **Traits pointillés** : liens entre fichiers (liens Markdown, `[[liens wiki]]`, chemins cités dans le texte, noms de skills mentionnés).
- **Cercle blanc autour** : un skill (dossier contenant un `SKILL.md`).
- **Nœud gris** : fichier iCloud pas encore téléchargé (☁️ dans les listes). Il n'est pas lu pendant l'analyse, pour ne pas déclencher des milliers de téléchargements.
- **Nœud « +N autres »** : pour rester fluide, la carte affiche au plus 4 000 éléments, répartis équitablement entre les zones, et 250 par dossier. Clic dessus pour voir la liste complète.

## Utilisation

| Action | Comment |
|---|---|
| Lire un fichier / ouvrir un dossier | Clic sur le nœud |
| Chercher partout | `/` puis taper : résultats de **toute l'arborescence**, même au-delà de ce que la carte affiche |
| Zoomer / se déplacer | Molette / glisser le fond |
| Épingler un nœud | Le glisser où tu veux |
| Plein écran | `F` |
| Vue sphère 3D / carte 2D | Bouton **◎ 3D** (le choix est retenu) |
| Fermer le panneau / la visionneuse | `Échap` |
| Média suivant / précédent | Flèches ← → dans la visionneuse |
| Revenir en arrière dans le panneau | Bouton ← ou touche Retour arrière |
| Relire les dossiers après un changement | Bouton ↻ |
| Réglages | Bouton ⚙ |

Dans le panneau de droite :

- **Dossier** : barre de filtres (recherche dans le dossier **et tous ses sous-dossiers**, option « dans le contenu des fichiers », tri, taille, type), **galerie des médias** (images, GIF / WebP animés, vidéos), **galerie des documents** (aperçu de la 1re page), puis la liste du contenu. Un élément correspond à la recherche si les mots sont dans son nom ou dans le nom d'un de ses dossiers parents.
- **Visionneuse** : images, vidéos et documents s'ouvrent **par-dessus** le panneau ; ← → pour passer au suivant, Échap pour revenir exactement où tu étais.
- **Vidéos AVI, MKV, WMV…** : si **ffmpeg** est installé (`brew install ffmpeg`), elles sont converties au premier visionnage puis gardées en cache. Sinon, un bouton les ouvre dans l'app du Mac.
- **Documents** : PDF dans le lecteur du navigateur, texte et Markdown mis en forme, Word / RTF / ODT convertis en page lisible ; pour Excel, PowerPoint, Pages, Numbers et Keynote, l'aperçu + **Ouvrir avec l'app du Mac**.
- **Dossiers miroirs** (si l'archivage est activé) : le panneau d'un dossier de notes affiche aussi le contenu de son dossier miroir dans l'archive (marqué 🗄). La case **Inclure l'archive miroir** permet de le masquer. Sur la carte, les deux dossiers sont reliés par un lien « miroir ».
- **Catalogues** : les fiches des `catalog.yaml` (`path`, `link`, `series`, `description`…) et des `CATALOG.jsonl` (`rel`, `title`, `note`…) s'affichent sous les médias.
- **Markdown** mis en forme, **conversations** JSON en bulles, **bases SQLite** consultables, **fichiers secrets** (`.env`, `auth.json`) masqués.
- En bas : **« Fait référence à »** et **« Cité par »** pour naviguer de lien en lien.

## Mettre à jour avec Hermes

Le bouton **☤ Mettre à jour avec Hermes** demande à Hermes de mettre à jour tes notes **à partir de l'endroit où tu es** : un dossier (et ses sous-dossiers), une note, ou le dossier d'un autre fichier. La consigne par défaut lui demande de relier les notes qui parlent du même sujet, d'ajouter des `tags:` et de tenir un `INDEX.md` par dossier, avec **uniquement des liens Markdown classiques** (`[Titre](chemin/relatif.md)`), sans doublon. Elle lui interdit de sortir du dossier, de toucher à autre chose que des `.md`, et de supprimer, renommer ou réécrire du contenu. La consigne est modifiable ; ta version est retenue.

Déroulement :
1. **Sauvegarde** des `.md` concernés (dans `~/Library/Application Support/hermes-map/backups/<date>`).
2. Hermes est lancé avec `hermes [-p profil] chat --query-file -`. Son journal s'affiche en direct, et **sur la carte** les nœuds pulsent selon ce qu'il fait (👁 lu, ✏️ écrit, ✨ créé) ; les nouveaux liens se dessinent aussitôt.
3. À la fin : la liste des fichiers modifiés / créés / supprimés, avec le détail ligne par ligne.
4. **↶ Annuler** remet les notes comme avant.

**Profil** : si un profil Hermes porte le nom d'une zone (profil `pro` pour la zone `Pro`), il est présélectionné quand tu travailles dans cette zone ; sinon c'est le profil choisi dans les Réglages.

## Reclasser un fichier mal rangé

Coche un ou plusieurs fichiers (case sur la vignette ou la ligne), puis choisis une action dans la barre du bas.

### ✋ Déplacer / renommer (manuel, sans IA)
- Choisis la destination (suggestions au fil de la frappe, raccourcis vers les dossiers d'archive), et renomme si besoin. Le dossier est créé s'il n'existe pas.
- **Mise à jour des index** : la fiche du `catalog.yaml` suit le fichier, la ligne du `CATALOG.jsonl` est mise à jour, une ligne est ajoutée au journal `LOG.md` de l'archive (s'il existe), et les liens des notes qui pointaient vers l'ancien emplacement sont corrigés.
- **↶ Annuler** remet fichiers et index comme avant.

### ☤ Reclasser avec Hermes (analyse + rangement)
Hermes analyse chaque fichier, choisit le bon dossier, le renomme lisiblement et le range (dans l'archive, au chemin miroir, si l'archivage est activé), puis met à jour les index. S'il n'est pas sûr, il laisse le fichier en place et explique pourquoi. Avant, Hermes Map sauvegarde les index (`.md`, `.yaml`, `.jsonl`) et note l'empreinte de chaque fichier ; après, il montre où chaque fichier est parti, même renommé, et **↶ Annuler** remet tout en place.

Dans les Réglages avancés, tu peux indiquer des **fichiers de règles** que Hermes doit suivre (ex. `Accueil/CONVENTIONS.md`), le **dossier de travail** du reclassement et les dossiers à ne pas sauvegarder. Pour « regarder » une image, le modèle du profil doit savoir lire les images.

## Options de la ligne de commande (facultatif)

```
python3 hermes_map.py
python3 hermes_map.py ~/Notes --only "*"
python3 hermes_map.py --port 9000 --hidden
python3 hermes_map.py --no-hermes
```

- un dossier en argument : l'explore directement, sans écran d'accueil (les réglages enregistrés ne changent pas) ;
- `--only` : dossiers de premier niveau à afficher (motifs séparés par des virgules ; `"*"` = tout) ;
- `--hidden` : affiche aussi les fichiers cachés ; `--reveal-secrets` : affiche les clés API en clair ;
- `--hermes /chemin/vers/hermes`, `--hermes-args "…"` : commande Hermes et options ajoutées à `hermes chat` ;
- `--no-hermes` : retire les fonctions Hermes.

## Sécurité et confidentialité

- Le serveur n'écoute que sur ta machine (`127.0.0.1`). Les actions qui écrivent sont protégées par un jeton secret propre à chaque lancement : un autre site ouvert dans ton navigateur ne peut pas les déclencher.
- L'app n'envoie rien sur Internet (toutes les bibliothèques sont incluses). Quand tu lances Hermes, c'est lui qui contacte le modèle configuré dans ses réglages.
- L'exploration est en lecture seule. Seules trois actions écrivent, toujours à ta demande, après sauvegarde et avec annulation possible : **Mettre à jour avec Hermes**, **Reclasser avec Hermes** et **Déplacer / renommer**. Activer l'archivage crée le dossier d'archivage s'il n'existe pas.
- Les miniatures et vidéos converties sont en cache dans `~/Library/Caches/hermes-map` ; tu peux supprimer ce dossier à tout moment.

## Soutenir le projet

Hermes Map est gratuit et open source. S'il t'est utile, tu peux [soutenir le projet sur GitHub Sponsors](https://github.com/sponsors/AnthonyDpz) ☕ (lien aussi dans l'écran ⚙ Réglages). L'app reste entièrement utilisable sans soutien.

## Licence

MIT (voir `LICENSE`). Bibliothèques incluses : voir `THIRD_PARTY_NOTICES.md`.

# Notes pour les assistants IA

## Numéro de version

Le numéro de version est affiché en haut à gauche, à côté du nom de l'app
(`static/index.html`, élément `#appVersion`). Il est commun aux versions Python et Rust.

À chaque modification de l'app, mettre ce numéro à jour :
- changement **majeur** (nouvelle fonctionnalité ou correctif important) : **+0.01** (ex. v0.1101 → v0.1201) ;
- changement **mineur** : **+0.0001** (ex. v0.1101 → v0.1102).

Toujours écrire la version avec 4 décimales.

Historique :
- v0.1000 : version de départ (état du 27 octobre 2026)
- v0.1100 : fichiers ignorés (réglage + bouton « Ignorés ») — majeur
- v0.1101 : numéro de version affiché à côté du nom de l'app — mineur
- v0.1102 : build_app.sh — correctif de signature (attributs iCloud) et option --rust — mineur
- v0.1202 : modification des fichiers texte dans le panneau (✎ Modifier, aperçu, Cmd+S, conflit, annulation) — majeur

#!/bin/zsh
# Double-clic pour lancer Hermes Map depuis les sources (sans construire l'app).
# Ferme cette fenêtre de Terminal pour arrêter l'application.
cd "$(dirname "$0")"
python3 hermes_map.py

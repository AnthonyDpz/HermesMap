//! Fichiers de l'interface (dossier static/ du projet), intégrés dans l'exécutable.
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../static/"]
pub struct Static;

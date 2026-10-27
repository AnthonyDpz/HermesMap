//! Consignes envoyées à Hermes (générées depuis hermes_runner.py : ne pas modifier à la main).

pub const DEFAULT_PROMPT_DIR: &str = r#"Tu travailles dans le dossier « {nom} » de ma base de connaissances Hermes :
{chemin}

Mets à jour les fichiers Markdown (.md) de ce dossier ET de tous ses sous-dossiers :
1. Relie les notes qui parlent du même sujet, de la même personne, du même lieu ou du même projet.
2. Dans chaque note, ajoute ou complète un en-tête YAML (entre deux lignes ---) avec un champ `tags:` : 3 à 6 tags courts, en minuscules, sans accents, réutilise en priorité les tags déjà présents dans le dossier.
3. Crée ou mets à jour un fichier INDEX.md dans chaque dossier qui contient des notes : un court résumé du contenu du dossier, puis la liste de ses fichiers et sous-dossiers avec un lien vers chacun.

Format des liens (impératif) :
- Utilise UNIQUEMENT des liens Markdown classiques : [Titre lisible](chemin/relatif/vers/la-note.md). N'écris JAMAIS de liens [[…]].
- Le chemin est relatif à la note qui contient le lien (ex. ../lieux/paris.md). Si le chemin contient des espaces, entoure-le de < > : [Titre](<../mon dossier/note.md>).
- Le texte du lien est le titre de la note visée (sa ligne « # Titre »), pas le nom du fichier.
- Avant d'ajouter un lien, vérifie qu'il n'existe pas déjà dans la note (même cible, quelle que soit la forme) : aucun doublon.
- Ne modifie pas les liens existants. Respecte les conventions décrites dans les INDEX.md / README.md du dossier.

Règles strictes :
- Ne travaille QUE dans ce dossier et ses sous-dossiers, n'en sors jamais.
- Ne modifie que des fichiers .md ; ne touche à aucun autre type de fichier.
- Ne supprime, ne renomme et ne déplace aucun fichier ni dossier.
- Conserve tout le texte existant : ajoute des liens et des tags, ne réécris pas et ne résume pas le contenu des notes.
- N'exécute aucune commande qui modifie le système ; n'utilise pas Internet.
À la fin, donne la liste des fichiers que tu as créés ou modifiés."#;

pub const DEFAULT_PROMPT_FILE: &str = r#"Tu travailles sur la note « {nom} » de ma base de connaissances Hermes :
{chemin}

Mets à jour CE fichier uniquement :
1. Ajoute des liens vers les autres notes .md du même dossier ({dossier}) et de ses sous-dossiers qui parlent du même sujet.
2. Ajoute ou complète un en-tête YAML (entre deux lignes ---) avec un champ `tags:` : 3 à 6 tags courts, en minuscules, sans accents.

Format des liens (impératif) :
- UNIQUEMENT des liens Markdown classiques [Titre lisible](chemin/relatif.md), jamais de [[…]] ; chemin relatif à cette note, entre < > s'il contient des espaces.
- Texte du lien = titre de la note visée (sa ligne « # Titre »).
- Aucun doublon : ne rajoute pas un lien vers une note déjà liée dans ce fichier, quelle que soit la forme. Ne modifie pas les liens existants.

Règles strictes :
- Ne modifie QUE ce fichier ; tu peux lire les autres notes du dossier mais pas les modifier.
- Conserve tout le texte existant : ajoute des liens et des tags, ne réécris pas le contenu.
- Ne supprime, ne renomme et ne déplace aucun fichier ; n'utilise pas Internet.
À la fin, résume ce que tu as changé."#;

pub const DEFAULT_PROMPT_DIR_EN: &str = r#"You are working in the “{nom}” folder of my Hermes knowledge base:
{chemin}

Update the Markdown (.md) files in this folder AND all its subfolders:
1. Link notes that are about the same subject, person, place or project.
2. In each note, add or complete a YAML header (between two --- lines) with a `tags:` field: 3 to 6 short tags, lowercase, without accents; reuse tags already present in the folder first.
3. Create or update an INDEX.md file in each folder that contains notes: a short summary of the folder's contents, then the list of its files and subfolders with a link to each.

Link format (mandatory):
- Use ONLY standard Markdown links: [Readable title](relative/path/to/the-note.md). NEVER write [[…]] links.
- The path is relative to the note that contains the link (e.g. ../places/paris.md). If the path contains spaces, wrap it in < >: [Title](<../my folder/note.md>).
- The link text is the title of the target note (its “# Title” line), not the file name.
- Before adding a link, check it doesn't already exist in the note (same target, whatever the form): no duplicates.
- Don't change existing links. Follow the conventions described in the folder's INDEX.md / README.md files.

Strict rules:
- Work ONLY in this folder and its subfolders; never leave it.
- Only change .md files; don't touch any other file type.
- Don't delete, rename or move any file or folder.
- Keep all existing text: add links and tags, don't rewrite or summarise the notes' content.
- Don't run any command that changes the system; don't use the Internet.
At the end, list the files you created or changed."#;

pub const DEFAULT_PROMPT_FILE_EN: &str = r#"You are working on the note “{nom}” of my Hermes knowledge base:
{chemin}

Update THIS file only:
1. Add links to the other .md notes in the same folder ({dossier}) and its subfolders that are about the same subject.
2. Add or complete a YAML header (between two --- lines) with a `tags:` field: 3 to 6 short tags, lowercase, without accents.

Link format (mandatory):
- ONLY standard Markdown links [Readable title](relative/path.md), never [[…]]; path relative to this note, wrapped in < > if it contains spaces.
- Link text = title of the target note (its “# Title” line).
- No duplicates: don't add a link to a note already linked in this file, whatever the form. Don't change existing links.

Strict rules:
- Change ONLY this file; you may read the other notes in the folder but not change them.
- Keep all existing text: add links and tags, don't rewrite the content.
- Don't delete, rename or move any file; don't use the Internet.
At the end, summarise what you changed."#;

pub const RECLASS_HEAD: &str = r#"Reclassement de fichiers mal rangés dans ma base de connaissances (racine : {racine}).

Fichiers à traiter ({nb}) :
{fichiers}

Pour CHAQUE fichier :
1. Analyse-le (regarde l'image / lis le document) : de quoi s'agit-il ? À quel dossier appartient-il vraiment ?"#;

pub const RECLASS_TAIL: &str = r#"3. Si tu n'es pas sûr de la destination ou du sujet, NE DÉPLACE PAS ce fichier : explique pourquoi et propose une destination.

Règles strictes :
- Ne déplace et ne renomme QUE les fichiers listés ci-dessus. Ne supprime rien.
- Ne modifie que des fichiers texte d'index (.md, .yaml, .jsonl) en plus de ces déplacements.
- N'utilise pas Internet.
À la fin, donne un tableau : ancien chemin → nouveau chemin (ou « laissé en place » + raison)."#;

pub const RECLASS_TAIL_EN: &str = r#"3. If you're not sure about the destination or the subject, DO NOT MOVE the file: explain why and suggest a destination.

Strict rules:
- Only move and rename the files listed above. Don't delete anything.
- Only change text index files (.md, .yaml, .jsonl) in addition to these moves.
- Don't use the Internet.
At the end, give a table: old path → new path (or “left in place” + reason)."#;

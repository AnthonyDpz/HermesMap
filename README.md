# Hermes Map

A visual explorer for your notes folders: a mind map of folders, files and the links between them, with a reading panel on the right. Built as a companion for the **Hermes** agent (`hermes` command), it also works without it on any folder of notes — Markdown, an Obsidian vault, iCloud Drive…

![Hermes Map](docs/screenshot.png)

> The interface is in French. 🇫🇷 Full guide: [LISEZMOI.md](LISEZMOI.md)

## Features

- **2D map and 3D sphere** of your folders, with one colour per top-level folder (“zone”).
- **Links between notes**: Markdown links, `[[wiki links]]`, file paths quoted in text, skill mentions.
- **Tags**: front-matter `tags:` and `#hashtags`, plus automatic tags from words that recur across branches.
- **Search everything**, including folders the map does not draw, and optionally inside file contents.
- **Reading panel**: rendered Markdown, image / GIF / video galleries, PDF and Office previews, SQLite tables, chat logs; secrets (`.env`, `auth.json`) are masked.
- **Optional archive folder**: keep notes in their folders and binaries in a mirrored archive tree (`Notes/trips/japan.md` ↔ `Archive/notes/trips/`); the map shows both together.
- **Move / rename** files by hand, with catalogue, log and note links updated — and undo.
- **With Hermes installed**: “Update with Hermes” (links, tags, INDEX.md from where you are) and “Reclassify with Hermes” (analyse, rename, file away), with live activity on the map, a backup before every run, a full diff and one-click undo.

## Install (macOS)

Requires Apple's command line tools (they also provide Python 3):

```
xcode-select --install
git clone https://github.com/AnthonyDpz/HermesMap.git
cd HermesMap
./macos/build_app.sh --install
```

Then launch **Hermes Map** from Spotlight. The first launch shows a setup screen: it detects Hermes and suggests folders to explore (iCloud Drive, Obsidian vaults, `~/.hermes`, Documents), and asks whether to centralise files in an archive folder. To read protected folders such as iCloud Drive, grant the app **Full Disk Access** when prompted.

## Run from source (macOS / Linux)

```
python3 hermes_map.py
```

Python 3.8+ only, no dependencies. Thumbnails and Office previews use macOS tools (`sips`, `qlmanage`, `textutil`); `ffmpeg`, if installed, converts AVI/MKV videos for playback.

## Privacy

The server listens on `127.0.0.1` only, write actions require a per-launch token, and nothing is sent to the internet. Browsing is read-only; files are only changed by the actions you start (Hermes update, reclassify, move), always after a backup and with undo. Settings live in `~/Library/Application Support/hermes-map/config.json`.

## Support

Hermes Map is free and open source. If it is useful to you, you can [sponsor the project on GitHub](https://github.com/sponsors/AnthonyDpz) ☕

## License

MIT — see [LICENSE](LICENSE). Bundled libraries: see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

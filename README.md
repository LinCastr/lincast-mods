# linex-mods

Modding manager vibecoded that works like Vortex mod manager because i'm lazy to mod on Linux.

Native Linux app (Rust). It finds your Steam games, shows the mods of each one and installs
new ones from an archive — the boring part of modding (unzip, find the right folder, copy)
done for you.

> Early work in progress. No releases yet. The interface is in Portuguese for now.

## What works today

- Finds Steam libraries (native and Flatpak Steam) and the installed games that have a mod
  loader: **MelonLoader**, **BepInEx** and **SMAPI** (Stardew Valley).
- Lists mods with name, version and author (from SMAPI `manifest.json`).
- Installs `.zip` archives (and `.7z`/`.rar` through `7z` or `bsdtar`), putting each file in
  the right folder for the loader. Reinstalling a mod keeps its `config.json`.
- Turns mods on and off without deleting them (moved to `.linex-disabled/` inside the game).
- Its own window (dark, minimal), with the game art from Steam's cache.
- Remembers which files each mod installed, so removing a mod sends all of it to the trash
  (shared libraries stay while another mod still uses them).
- Nexus "Mod Manager Download" button (`nxm://` links), free accounts included. The API key
  is kept in the system keyring (Secret Service). A click while the window is open goes to
  the open window.

## Planned

- Warning and fix for the Proton DLL override that MelonLoader/BepInEx need.
- Thunderstore support.

Run `linex-mods` with no arguments to open the window.

## Command line

```
linex-mods jogos
linex-mods mods <game>
linex-mods instalar <game> <archive>...
linex-mods ligar <game> <mod>
linex-mods desligar <game> <mod>
linex-mods remover <game> <mod>
```

`<game>` is the Steam AppID or part of the name.

## Build

```
cargo build --release
```

## License

GPL-3.0-or-later.

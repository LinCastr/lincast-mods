# Research: Amethyst Mod Manager

Source: https://github.com/ChrisDKN/Amethyst-Mod-Manager (Python + Qt, GPL-3.0, the same
license as this project, so parts can be ported with credit). Read on 2026-10-11, v2.5.4.

## What to reuse

- **`nxm://` link**: `nxm://<domain>/mods/<mod>/files/<file>?key=..&expires=..`. Free
  accounts can only download through this link (one-time key); premium accounts get a
  direct link from the API. Collections: `nxm://<domain>/collections/<slug>[/revisions/<id>]`.
- **Single instance**: the browser starts a new process with the link, which hands it to
  the open app over a Unix socket. Their real-world problem: the browser's process sometimes
  resolves a different socket path (Flatpak, lost `XDG_RUNTIME_DIR`), so they try several
  paths and fall back to `/tmp/<app>-<uid>.sock`. That process has no window, so they log
  everything to `logs/nxm.log`.
- **API key**: in the system keyring (Secret Service), with a file as fallback when
  D-Bus/keyring fails (common on SteamOS). They also have OAuth 2.0 + PKCE login, but it
  needs a `client_id` registered with the Nexus ("amethyst"); we would have to ask for ours.
- **Game detection**: Steam, Heroic, Lutris and Faugus.
- **Per-game rules**: `src/Games/<game>/` (36 definitions, "almost 100 games" counting the
  generic BepInEx/UE5 ones). A good map of what each game needs.
- **Stardew**: root files (SMAPI) can't be symlinks: the .NET apphost looks for the runtime
  next to the executable.

## Where we differ on purpose

- **Deployment**: they use a staging folder + hardlinks/symlinks/VFS and profiles, MO2
  style, with a "restore" that returns the game folder to its original state. Safer, much
  more complex. Here it is a direct copy for now, and disabling moves the mod to
  `.linex-disabled/`.
- **Interface**: theirs is dense, MO2 style. Ours is for people who have never modded.

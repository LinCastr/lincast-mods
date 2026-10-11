# linex-mods — notes for AI agents

Native Linux mod manager for Steam games (Vortex-like), Rust + Slint. Read this and
`README.md` before the first edit.

**Talk to the user in Brazilian Portuguese.** Code, comments, UI strings, commits and
docs are in English (some old comments are still in Portuguese; translate when touched).
Commit messages: short imperative English subject, no conventional-commit prefix needed.

## How to work here (not optional)

Most bugs come from rushed edits, not hard tasks.

1. **Read before writing.** Open the file you will change and the related ones.
2. **State the diagnosis before editing.** In 3–6 lines: what you understood, where the
   cause is (file and line) and what you will change. If the request can be read two
   ways, **ask** instead of guessing. A question costs one message; a wrong guess costs a
   session of undoing.
3. **Minimal scope.** Touch only what the request covers. No renaming, reordering,
   reformatting or "improving along the way". Unrequested refactors get rejected.
4. **One change at a time.** Edit, verify, then move on.
5. **Verify for real:** `cargo build` (and `cargo clippy`) with no new warnings; run the
   affected CLI path; for UI, a snapshot (see "Testing without clicking") when a display
   exists. Without verification, don't say it's done.
6. **Report honestly:** what changed, what you tested and what you did **not** test.
   Never write "fixed and tested" about something you only read. If a command, test or
   build fails, report every error clearly.
7. **Complex or multi-file task:** present the plan (scope, steps, files) and wait for the
   OK. Trivial isolated fixes can go straight in.

Absolute rules:
- **Pushing:** never push to `main`, force push, open releases or change repo settings
  unless the user asks. Pushing your own work to a `claude/...` branch is allowed and
  expected: in a cloud session, push after every commit so the work survives if the
  container breaks (a session once lost its network and its commits were stranded).
- **Never run a test with a safety guard removed against real paths.** To prove a test
  catches a bug, every path it can touch must be inside its own temporary HOME. Cloud
  containers run as root: a test that trashed `/etc` there killed the session.
- **Privacy:** never commit personal data, tokens, API keys, logs, or paths with a real
  home directory (`/home/<user>`). The repo is public.
- **Never overwrite user state** (`~/.config/linex-mods/`, `~/.local/share/linex-mods/`,
  the user's game folders and mods) while testing: use a fake `HOME`.
- Commands for the user to run: one per ```bash block, no leading `$`. The user's shell
  is fish, so bash-only syntax needs `bash -c '...'`.

## Visual: no "AI look"

Screens have been rejected before for looking "made by AI".

- Don't: emoji as icons, mockups of things that exist, "tip" cards with colored circle
  icons, everything in outlined boxes, cards inside cards, loose hex colors (colors come
  from `Theme`), marketing text (adjectives, exclamation marks, promises the app doesn't
  keep), gradients/glow/colored shadows/animations without a purpose.
- Do: one accent per screen (theme color only on what is on/selected), hierarchy through
  text rather than boxes, reuse the existing components in `ui/app.slint`, real controls
  that read state on open and apply immediately. Reference look: Steam, r2modman, the
  Claude app — minimal, dark, usable by anyone.

## Build and run

```sh
cargo build --release          # the installed desktop entry points to target/release/linex-mods
cargo run -- games             # CLI: games, mods, install, enable, disable, remove, unity, loader
```

Rust edition 2024, Slint 1.18 (`build.rs` compiles `ui/app.slint` with bundled
translations from `lang/`). The release profile uses LTO, so it takes ~2.5 min.

## Layout

| File | What |
| --- | --- |
| `src/main.rs` | CLI + entry. `#[macro_use] mod i18n;` must stay first (the `tr!` macro). |
| `src/gui.rs` | Everything the window does: state, callbacks, notifications, update check, DnD, watchers. |
| `ui/app.slint` | The whole UI. Global `Theme` colors set from Rust (`theme.rs`). |
| `src/steam.rs`, `vdf.rs` | Steam libraries, appmanifests, Proton detection, launch options, library art. |
| `src/games.rs` | `Loader` enum (SMAPI, MelonLoader, BepInEx), known games, loader detection. |
| `src/mods.rs` | Listing mods (SMAPI/Thunderstore manifests, DLLs), enable/disable, uninstall. |
| `src/install.rs` | Archive extraction (zip native; 7z/bsdtar for others) and file placement per loader. |
| `src/records.rs` | `~/.local/share/linex-mods/<appid>.json`: which files each mod installed. |
| `src/nexus.rs` | Nexus API v1, `nxm://` links, API key in the keyring (file fallback, 0600). |
| `src/meta.rs` | Mod icons (64px PNG cache) and cached Nexus mod info (6 h TTL). |
| `src/loaders.rs` | Installing MelonLoader / BepInEx (5 for Mono, 6 BE for IL2CPP) into Unity games. |
| `src/desktop.rs` | `.desktop` entry, icon, `x-scheme-handler/nxm` registration. |
| `src/ipc.rs` | Single instance: a second launch with an `nxm://` URL hands it over a Unix socket. |
| `src/i18n.rs` | `tr!("English", "Português")` for Rust strings; Slint uses `@tr()` + `lang/pt_BR/.../linex-mods.po`. |
| `src/theme.rs` | Default dark palette or colors from wallust/pywal `colors.json`. |
| `tests/cli.rs` | End-to-end CLI tests against a fake Steam library in a temporary HOME. |
| `docs/amethyst-research.md` | Research notes on Amethyst Mod Manager (how Nexus/loaders behave on Linux). |

## Rules learned the hard way

- Every new visible string: `@tr("...")` in Slint **and** an entry in the `.po` file;
  `tr!(en, pt)` in Rust. msgids must be unique in the `.po` (a duplicate breaks the build).
- `.desktop` `Exec=` must be unquoted (xdg-mime rejects quoted Exec). `current_exe()` can
  end in " (deleted)" after a rebuild; `desktop::exe_path` strips it. Only rewrite the
  entry when its Exec is stale, or test builds hijack the user's nxm handler.
- The GUI prefers XWayland (`gui::prefer_x11`) because winit has no drag & drop on Wayland;
  `WINIT_X11_SCALE_FACTOR=1` when there is no `Xft.dpi`, or the UI is scaled 1.33x.
- Never log the Nexus one-time download key (`NxmLink::redacted`).
- `records::put` ignores overlaps in shared dirs (`UserLibs/`), otherwise installing one
  mod drops another mod's record.
- Proton games need `WINEDLLOVERRIDES` (`winhttp=n,b` BepInEx, `version=n,b` MelonLoader).
- Flickable: use `content-*`, not the deprecated `viewport-*`.

## Testing without clicking

Env vars read by `gui.rs` for headless screenshots (needs a display):
`LINEX_SNAPSHOT=out.png` (save window and quit), `LINEX_GAME=<appid>`,
`LINEX_PAGE=1|2` (settings / add game), `LINEX_MESSAGE="a|b|c"`, `LINEX_PANEL=1`,
`LINEX_PROPS=<mod id>`, `LINEX_SORT_DESC=1`, `LINEX_SNAPSHOT_DELAY=<ms>`, `LINEX_APP_ID`.
A fake Steam library (fake `HOME` with `.local/share/Steam/steamapps/...`) works for
CLI tests. In a cloud session there is no display and no Steam: limit yourself to
`cargo build`, `cargo test`, `cargo clippy` and CLI runs against a fake library.

## Status (2026-10-11)

Done: games/loader detection, install/enable/disable/remove with records, Nexus
"Mod Manager Download" (nxm), notifications + bell, wallust theme, loader installer,
drag & drop, mod icons, English UI + pt_BR, right-click menu, properties, sort, update check.
Code comments are all in English. CI (`.github/workflows/ci.yml`) runs
`cargo clippy --all-targets -- -D warnings` and `cargo test`: keep both clean.

Security audit done (2026-10-11): archive symlinks/FIFOs skipped, archive names and record
paths that escape the game folder rejected, nxm links validated, the one-time key redacted
from error messages, IPC sockets owned and 0600, key file forced to 0600.

Not yet tested by the user with the mouse: right-click menu, real drag & drop, dragging
the scrollbar, loader install on a real game, copying the launch option.

Next / open:
- Thunderstore support (download + install packages, r2modman-style). Not started.
- Rewriting the first ~15 Portuguese commit messages needs a force push to the public
  repo: only with the user's explicit OK.
- No GitHub releases yet (user's choice). Don't create one.

//! End-to-end tests of the command line against a fake Steam library in a temporary
//! HOME: install, list, disable, enable and remove for each loader.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Home {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("linex-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join(".local/share/Steam/steamapps/common")).unwrap();
        Home(p)
    }

    fn steamapps(&self) -> PathBuf {
        self.0.join(".local/share/Steam/steamapps")
    }

    /// Adds a game to the fake library and returns its folder.
    fn game(&self, appid: u32, name: &str, files: &[&str]) -> PathBuf {
        let dir = self.steamapps().join("common").join(name);
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            let p = dir.join(f);
            if f.ends_with('/') {
                fs::create_dir_all(&p).unwrap();
            } else {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, "").unwrap();
            }
        }
        fs::write(
            self.steamapps().join(format!("appmanifest_{appid}.acf")),
            format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{name}\"\n}}\n"),
        )
        .unwrap();
        dir
    }

    fn zip(&self, name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let p = self.0.join(name);
        let mut z = zip::ZipWriter::new(fs::File::create(&p).unwrap());
        for (path, data) in entries {
            z.start_file(*path, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        p
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_linex-mods"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LANG", "C")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        (out.status.success(), text)
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, text) = self.run(args);
        assert!(ok, "linex-mods {args:?} failed:\n{text}");
        text
    }

    fn records(&self, appid: u32) -> serde_json::Value {
        let p = self.0.join(format!(".local/share/linex-mods/{appid}.json"));
        serde_json::from_str(&fs::read_to_string(p).unwrap_or_else(|_| "{}".into())).unwrap()
    }

    fn trash_has(&self, name: &str) -> bool {
        self.0.join(".local/share/Trash/files").join(name).exists()
    }
}

fn record_names(v: &serde_json::Value) -> Vec<String> {
    v["mods"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect()
}

#[test]
fn smapi_mod_lifecycle() {
    let h = Home::new("smapi");
    let game = h.game(413150, "Stardew Valley", &["StardewModdingAPI.dll", "Mods/"]);
    let manifest = r#"{ "Name": "Cool Mod", "Author": "Someone", "Version": "1.2.0", "UniqueID": "a.b", }"#;
    let zip = h.zip("Cool Mod-123-1-2-0-1700000000.zip", &[("CoolMod/manifest.json", manifest), ("CoolMod/a.dll", "x")]);

    let out = h.ok(&["games"]);
    assert!(out.contains("413150") && out.contains("SMAPI"), "{out}");

    h.ok(&["install", "413150", zip.to_str().unwrap()]);
    assert!(game.join("Mods/CoolMod/manifest.json").is_file());
    assert_eq!(record_names(&h.records(413150)), ["Cool Mod"]);
    let out = h.ok(&["mods", "stardew"]);
    assert!(out.contains("on   Cool Mod 1.2.0 — Someone"), "{out}");

    // The user's config.json survives a reinstall.
    fs::write(game.join("Mods/CoolMod/config.json"), "{\"mine\":1}").unwrap();
    h.ok(&["install", "413150", zip.to_str().unwrap()]);
    assert_eq!(fs::read_to_string(game.join("Mods/CoolMod/config.json")).unwrap(), "{\"mine\":1}");
    assert_eq!(record_names(&h.records(413150)), ["Cool Mod"]);

    h.ok(&["disable", "413150", "CoolMod"]);
    assert!(!game.join("Mods/CoolMod").exists());
    assert!(game.join(".linex-disabled/Mods/CoolMod/manifest.json").is_file());
    assert!(h.ok(&["mods", "413150"]).contains("off  Cool Mod"));

    // Reinstalling a disabled mod keeps it disabled.
    h.ok(&["install", "413150", zip.to_str().unwrap()]);
    assert!(!game.join("Mods/CoolMod").exists());

    h.ok(&["enable", "413150", "CoolMod"]);
    assert!(game.join("Mods/CoolMod/manifest.json").is_file());

    h.ok(&["remove", "413150", "CoolMod"]);
    assert!(!game.join("Mods/CoolMod").exists());
    assert!(h.trash_has("CoolMod"));
    assert!(record_names(&h.records(413150)).is_empty());
}

#[test]
fn smapi_package_with_several_mods_installs_the_parent_folder() {
    let h = Home::new("smapi-pack");
    let game = h.game(413150, "Stardew Valley", &["StardewModdingAPI.dll", "Mods/"]);
    let zip = h.zip(
        "Big.zip",
        &[
            ("Big Pack/[CP] Big/manifest.json", r#"{"Name":"Big CP"}"#),
            ("Big Pack/[CC] Big/manifest.json", r#"{"Name":"Big CC"}"#),
        ],
    );
    h.ok(&["install", "413150", zip.to_str().unwrap()]);
    assert!(game.join("Mods/Big Pack/[CP] Big/manifest.json").is_file());
    assert!(game.join("Mods/Big Pack/[CC] Big/manifest.json").is_file());
}

#[test]
fn melonloader_shared_library_outlives_the_first_mod() {
    let h = Home::new("melon");
    let game = h.game(1657630, "Slime Rancher 2", &["MelonLoader/", "Mods/", "SlimeRancher2.exe"]);
    let a = h.zip("ModA.zip", &[("ModA.dll", "MelonLoader"), ("UserLibs/Shared.dll", "x")]);
    let b = h.zip("ModB.zip", &[("ModB.dll", "MelonLoader"), ("UserLibs/Shared.dll", "x")]);
    h.ok(&["install", "1657630", a.to_str().unwrap()]);
    h.ok(&["install", "1657630", b.to_str().unwrap()]);
    assert!(game.join("Mods/ModA.dll").is_file() && game.join("Mods/ModB.dll").is_file());
    assert_eq!(record_names(&h.records(1657630)), ["ModA", "ModB"]);

    h.ok(&["remove", "1657630", "ModA.dll"]);
    assert!(!game.join("Mods/ModA.dll").exists());
    assert!(game.join("UserLibs/Shared.dll").is_file(), "ModB still needs it");

    h.ok(&["remove", "1657630", "ModB.dll"]);
    assert!(!game.join("UserLibs/Shared.dll").exists());
    assert!(game.join("UserLibs").is_dir() || !game.join("UserLibs").exists());
}

#[test]
fn bepinex_loose_dll_gets_its_own_folder_and_a_warning_for_the_wrong_loader() {
    let h = Home::new("bepinex");
    let game = h.game(3405340, "Megabonk", &["BepInEx/core/", "Megabonk.exe", "UnityPlayer.dll"]);
    let zip = h.zip("FastRun-1-0.zip", &[("FastRun.dll", "uses BepInEx"), ("README.md", "docs")]);
    h.ok(&["install", "megabonk", zip.to_str().unwrap()]);
    assert!(game.join("BepInEx/plugins/FastRun/FastRun.dll").is_file());
    assert!(!game.join("BepInEx/plugins/FastRun/README.md").exists());
    assert!(h.ok(&["mods", "megabonk"]).contains("FastRun"));

    let melon = h.zip("Wrong.zip", &[("Wrong.dll", "MelonLoader")]);
    let out = h.ok(&["install", "megabonk", melon.to_str().unwrap()]);
    assert!(out.contains("warning: this mod was made for MelonLoader"), "{out}");

    h.ok(&["remove", "megabonk", "FastRun"]);
    assert!(!game.join("BepInEx/plugins/FastRun").exists());
}

#[test]
fn unity_games_without_a_loader_are_listed() {
    let h = Home::new("unity");
    h.game(100, "Mono Game", &["Game.exe", "UnityPlayer.dll"]);
    h.game(200, "Native Game", &["game.x86_64"]);
    let out = h.ok(&["unity"]);
    assert!(out.contains("Mono Game") && out.contains("[Mono]"), "{out}");
    assert!(!out.contains("Native Game"));
    // Without a loader it is not a "moddable" game yet.
    assert!(!h.ok(&["games"]).contains("Mono Game"));
}

#[test]
fn bad_input_is_rejected() {
    let h = Home::new("bad");
    h.game(413150, "Stardew Valley", &["StardewModdingAPI.dll", "Mods/"]);
    for args in [
        &["remove", "413150", "../x"][..],
        &["disable", "413150", ".."],
        &["enable", "413150", "missing"],
        &["mods", "no such game"],
        &["frobnicate"],
    ] {
        let (ok, text) = h.run(args);
        assert!(!ok, "{args:?} should fail:\n{text}");
    }
    let not_a_mod = h.zip("Empty.zip", &[("readme.txt", "hi")]);
    let (ok, text) = h.run(&["install", "413150", not_a_mod.to_str().unwrap()]);
    assert!(!ok && text.contains("manifest.json"), "{text}");
}

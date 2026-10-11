//! Instala um mod a partir de um arquivo (.zip nativo; .7z/.rar pelo 7z ou bsdtar).

use crate::games::{Game, Loader};
use crate::records::{self, NexusSource, Record};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pasta temporária apagada ao sair do escopo.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let base = std::env::temp_dir();
        let pid = std::process::id();
        for n in 0..1000u32 {
            let p = base.join(format!("linex-mods-{pid}-{n}"));
            if fs::create_dir(&p).is_ok() {
                return Ok(TempDir(p));
            }
        }
        bail!("{}", tr!("couldn't create a temporary folder in {}", "não consegui criar pasta temporária em {}", base.display()))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive).with_context(|| tr!("opening {}", "abrindo {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).context(tr!("the file is not a valid .zip", "o arquivo não é um .zip válido"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // enclosed_name recusa caminhos que saem da pasta (../, absolutos).
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
    }
    Ok(())
}

fn have(cmd: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {cmd} >/dev/null 2>&1")])
        .status()
        .is_ok_and(|s| s.success())
}

fn extract_external(archive: &Path, dest: &Path) -> Result<()> {
    let ok = if have("7z") {
        Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest.display()))
            .arg(archive)
            .output()?
            .status
            .success()
    } else if have("bsdtar") {
        Command::new("bsdtar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()?
            .success()
    } else {
        bail!("{}", tr!("to open .7z/.rar files, install the 7zip (or libarchive) package", "para abrir .7z/.rar instale o pacote 7zip (ou libarchive)"));
    };
    if !ok {
        bail!("{}", tr!("couldn't extract {}", "falha ao extrair {}", archive.display()));
    }
    Ok(())
}

pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let ext = archive
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "zip" {
        extract_zip(archive, dest)
    } else {
        extract_external(archive, dest)
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)?.flatten() {
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst).with_context(|| tr!("copying to {}", "copiando para {}", dst.display()))?;
    }
    Ok(())
}

fn is_doc(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".md") || n.ends_with(".txt") || n.ends_with(".url") || n == "license" || n == "readme"
}

/// SMAPI: cada pasta com manifest.json é um mod. Se vários mods dividem uma pasta-mãe
/// (ex.: "Ridgeside Village/[CP] ..., [CC] ..."), instala a pasta-mãe inteira.
fn install_smapi(game: &Game, root: &Path, stem: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut mdirs: Vec<PathBuf> = files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n.eq_ignore_ascii_case("manifest.json")))
        .filter_map(|f| f.parent().map(Path::to_path_buf))
        .collect();
    if mdirs.is_empty() {
        bail!("{}", tr!("no manifest.json found: this doesn't look like a SMAPI mod", "não achei manifest.json: isto não parece um mod do SMAPI"));
    }
    // Só as pastas de topo (mod dentro de mod é parte do de fora).
    mdirs.sort();
    let mut tops: Vec<PathBuf> = Vec::new();
    for d in mdirs {
        if !tops.iter().any(|t| d.starts_with(t)) {
            tops.push(d);
        }
    }
    // Ancestral comum.
    let mut common = tops[0].clone();
    for t in &tops[1..] {
        while !t.starts_with(&common) {
            common.pop();
        }
    }
    let units: Vec<(PathBuf, String)> = if common == root {
        if tops.len() == 1 {
            // manifest.json na raiz do arquivo: a pasta leva o nome do arquivo.
            vec![(root.to_path_buf(), stem.to_string())]
        } else {
            tops.iter().map(|t| (t.clone(), name_of(t))).collect()
        }
    } else if tops.len() == 1 || name_of(&common).eq_ignore_ascii_case("Mods") {
        tops.iter().map(|t| (t.clone(), name_of(t))).collect()
    } else {
        vec![(common.clone(), name_of(&common))]
    };

    let mods_dir = game.mods_dir();
    let mut done = Vec::new();
    for (src, name) in units {
        replace_dir(&src, &mods_dir.join(&name), &game.disabled_dir().join(&name))?;
        done.push(format!("{}/{name}", game.loader.mods_dir()));
    }
    Ok(done)
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Troca a pasta do mod pela nova, mantendo os config.json que o usuário já tinha.
/// Se o mod estava desligado, a versão nova continua desligada.
fn replace_dir(src: &Path, enabled_dst: &Path, disabled_dst: &Path) -> Result<()> {
    let dst = if disabled_dst.exists() && !enabled_dst.exists() { disabled_dst } else { enabled_dst };
    let mut saved = Vec::new();
    if dst.exists() {
        let mut files = Vec::new();
        walk(dst, &mut files);
        for f in files {
            if f.file_name().is_some_and(|n| n == "config.json") {
                let rel = f.strip_prefix(dst).unwrap().to_path_buf();
                saved.push((rel, fs::read(&f)?));
            }
        }
        fs::remove_dir_all(dst).with_context(|| tr!("removing the old version in {}", "removendo versão antiga em {}", dst.display()))?;
    }
    copy_tree(src, dst)?;
    for (rel, data) in saved {
        let p = dst.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, data)?;
    }
    Ok(())
}

/// MelonLoader/BepInEx: pastas com nome conhecido vão para o lugar delas; DLL solta
/// vai para a pasta de mods.
fn install_unity(game: &Game, root: &Path, stem: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let dir = game.dir();
    let known: &[&str] = match game.loader {
        Loader::MelonLoader => &["Mods", "Plugins", "UserLibs", "UserData", "MelonLoader"],
        _ => &["BepInEx", "plugins", "patchers", "config", "core"],
    };
    let mut placed: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut loose: Vec<PathBuf> = Vec::new();
    // Pasta própria de um mod de BepInEx solto: vira uma unidade só no registro.
    let mut unit_dir: Option<PathBuf> = None;
    for f in &files {
        let rel = f.strip_prefix(root).unwrap();
        let comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        let hit = comps[..comps.len() - 1]
            .iter()
            .position(|c| known.iter().any(|k| k.eq_ignore_ascii_case(c)));
        if let Some(i) = hit {
            let rest: PathBuf = comps[i..].iter().collect();
            let dst = match game.loader {
                Loader::BepInEx if !comps[i].eq_ignore_ascii_case("BepInEx") => dir.join("BepInEx").join(rest),
                _ => dir.join(rest),
            };
            placed.push((f.clone(), dst));
        } else {
            loose.push(f.clone());
        }
    }
    let loose_dlls: Vec<&PathBuf> = loose
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")))
        .collect();
    match game.loader {
        Loader::MelonLoader => {
            for f in loose_dlls {
                placed.push((f.clone(), game.mods_dir().join(f.file_name().unwrap())));
            }
        }
        _ => {
            // Mod de BepInEx solto: tudo numa pasta própria em plugins/.
            if !loose_dlls.is_empty() {
                unit_dir = Some(game.mods_dir().join(stem));
                for f in &loose {
                    if f.file_name().is_some_and(|n| is_doc(&n.to_string_lossy())) {
                        continue;
                    }
                    let rel = f.strip_prefix(root).unwrap();
                    placed.push((f.clone(), game.mods_dir().join(stem).join(rel)));
                }
            }
        }
    }
    if placed.is_empty() {
        bail!("{}", tr!("nothing to install found (no DLL or {} folder)", "não achei nada para instalar (nenhuma DLL nem pasta do {})", game.loader.label()));
    }
    let rel = |p: &Path| p.strip_prefix(dir).unwrap_or(p).to_string_lossy().to_string();
    let mut names = Vec::new();
    if let Some(u) = &unit_dir {
        names.push(rel(u));
    }
    for (src, dst) in placed {
        // DLL que estava desligada: a versão nova substitui e continua desligada.
        let disabled = game.dir().join(".linex-disabled").join(rel(&dst));
        let target = if disabled.is_file() && !dst.exists() { disabled } else { dst.clone() };
        copy_tree(&src, &target)?;
        if unit_dir.as_ref().is_none_or(|u| !dst.starts_with(u)) {
            names.push(rel(&dst));
        }
    }
    Ok(names)
}

/// Para qual loader as DLLs do arquivo foram feitas. Uma DLL .NET guarda os nomes
/// dos assemblies que usa em texto puro, então basta procurar os nomes: mod de
/// MelonLoader cita "MelonLoader", de BepInEx cita "BepInEx". Ambíguo = None.
fn dll_loader(root: &Path) -> Option<Loader> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let has = |data: &[u8], needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
    let (mut melon, mut bepinex, mut smapi) = (0, 0, 0);
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll"))) {
        let Ok(data) = fs::read(f) else { continue };
        let (m, b, s) = (has(&data, b"MelonLoader"), has(&data, b"BepInEx"), has(&data, b"StardewModdingAPI"));
        melon += (m && !b) as u32;
        bepinex += (b && !m) as u32;
        smapi += s as u32;
    }
    match (melon > 0, bepinex > 0, smapi > 0) {
        (true, false, false) => Some(Loader::MelonLoader),
        (false, true, false) => Some(Loader::BepInEx),
        (false, false, true) => Some(Loader::Smapi),
        _ => None,
    }
}

/// Nome "limpo" do arquivo: tira a extensão e o sufixo de IDs do Nexus
/// ("Mod Legal-123-1-0-1700000000" → "Mod Legal").
pub fn clean_stem(archive: &Path) -> String {
    let stem = archive.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let parts: Vec<&str> = stem.split('-').collect();
    let mut keep = parts.len();
    while keep > 1 && parts[keep - 1].chars().all(|c| c.is_ascii_digit()) {
        keep -= 1;
    }
    let s = parts[..keep].join("-").trim().to_string();
    if s.is_empty() { stem } else { s }
}

/// Instala e registra. `name` e `nexus` vêm do download pelo Nexus; para arquivo
/// local o nome sai do próprio arquivo.
pub fn install_archive(
    game: &Game,
    archive: &Path,
    name: Option<String>,
    nexus: Option<NexusSource>,
) -> Result<Record> {
    let tmp = TempDir::new()?;
    extract(archive, &tmp.0)?;
    let stem = clean_stem(archive);
    let warning = dll_loader(&tmp.0).filter(|l| *l != game.loader).map(|l| {
        tr!(
            "this mod was made for {}, but the game uses {}: it won't load. Look for the {} version on the mod page.",
            "este mod foi feito para {}, mas o jogo usa {}: ele não vai carregar. Procure a versão para {} na página do mod.",
            l.label(),
            game.loader.label(),
            game.loader.label()
        )
    });
    let files = match game.loader {
        Loader::Smapi => install_smapi(game, &tmp.0, &stem),
        Loader::MelonLoader | Loader::BepInEx => install_unity(game, &tmp.0, &stem),
    }?;
    let rec = Record {
        name: name.unwrap_or(stem),
        files,
        archive: archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        nexus,
        installed_at: records::now(),
        warning,
    };
    let mut recs = records::load(game.steam.appid);
    recs.put(rec.clone());
    records::save(game.steam.appid, &recs)?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limpa_nome_do_nexus() {
        assert_eq!(clean_stem(Path::new("Continuous Sprint-7-1-0-1665850896.zip")), "Continuous Sprint");
        assert_eq!(clean_stem(Path::new("Rancher's Titles Version 1.0.0-155-1-0-0-1778287552.zip")), "Rancher's Titles Version 1.0.0");
        assert_eq!(clean_stem(Path::new("SimpleMod.zip")), "SimpleMod");
    }
}

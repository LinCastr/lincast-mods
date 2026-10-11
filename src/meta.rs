//! Mod icons and Nexus info (current version, picture), cached on disk.
//!
//! Icons come from the mod folder (`icon.png`, shipped by Thunderstore packages) or from
//! the mod's main picture on the Nexus. Both are shrunk to 64×64 PNGs in
//! ~/.cache/linex-mods/icons, so a list of a hundred mods stays light.

use crate::steam;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// How long Nexus info is reused before asking again.
const INFO_TTL_SECS: u64 = 6 * 3600;
const ICON_PX: u32 = 64;

fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| steam::home().join(".cache"))
        .join("linex-mods")
}

pub fn icons_dir() -> PathBuf {
    cache_root().join("icons")
}

/// Cached icon of a Nexus mod (may not exist yet).
pub fn nexus_icon(domain: &str, mod_id: u64) -> PathBuf {
    icons_dir().join(format!("nexus-{domain}-{mod_id}.png"))
}

/// Cached icon made from a mod's own `icon.png` (key: path + modification time,
/// so a new version of the mod gets a new icon).
pub fn local_icon(src: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    if let Ok(t) = fs::metadata(src).and_then(|m| m.modified()) {
        t.hash(&mut h);
    }
    icons_dir().join(format!("local-{:016x}.png", h.finish()))
}

/// Shrinks any image (png/jpg/webp/gif) to a square 64×64 PNG, cropping the middle.
pub fn make_thumbnail(bytes: &[u8], out: &Path) -> Result<()> {
    let img = image::load_from_memory(bytes).context("unreadable image")?;
    let thumb = img.resize_to_fill(ICON_PX, ICON_PX, image::imageops::FilterType::Triangle);
    fs::create_dir_all(out.parent().unwrap())?;
    let tmp = out.with_extension("png.tmp");
    thumb.save_with_format(&tmp, image::ImageFormat::Png)?;
    fs::rename(&tmp, out)?;
    Ok(())
}

pub fn ensure_local_icon(src: &Path) -> Option<PathBuf> {
    let out = local_icon(src);
    if out.is_file() {
        return Some(out);
    }
    let bytes = fs::read(src).ok()?;
    make_thumbnail(&bytes, &out).ok()?;
    Some(out)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NexusInfo {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub fetched: u64,
}

fn info_path() -> PathBuf {
    cache_root().join("nexus-info.json")
}

pub fn load_info() -> HashMap<String, NexusInfo> {
    fs::read_to_string(info_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_info(map: &HashMap<String, NexusInfo>) {
    let p = info_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    if let Ok(s) = serde_json::to_string(map) {
        let _ = fs::write(p, s);
    }
}

pub fn info_key(domain: &str, mod_id: u64) -> String {
    format!("{domain}/{mod_id}")
}

pub fn is_fresh(info: &NexusInfo) -> bool {
    crate::records::now().saturating_sub(info.fetched) < INFO_TTL_SECS
}

/// Downloads the Nexus picture and stores the thumbnail. Plain GET, no API key.
pub fn fetch_nexus_icon(url: &str, out: &Path) -> Result<()> {
    let resp = ureq::AgentBuilder::new()
        .user_agent(concat!("linex-mods/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(std::time::Duration::from_secs(15))
        .build()
        .get(url)
        .call()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader().take(20 * 1024 * 1024), &mut bytes)?;
    make_thumbnail(&bytes, out)
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_thumbnail() {
        let dir = std::env::temp_dir().join(format!("linex-thumb-{}", std::process::id()));
        let src = image::RgbaImage::from_pixel(300, 100, image::Rgba([200, 30, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = dir.join("t.png");
        make_thumbnail(&png, &out).unwrap();
        let t = image::open(&out).unwrap();
        assert_eq!((t.width(), t.height()), (64, 64));
        fs::remove_dir_all(dir).unwrap();
    }
}

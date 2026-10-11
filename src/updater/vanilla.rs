//! Vanilla version installation: fetch the version metadata, the client
//! jar, the asset index and every asset object into the standard layout
//! (`versions/<id>/`, `assets/`), with SHA-1 verification and exact
//! progress fractions.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::lang::{tr, tr_fmt, Language};
use crate::util::net;

use super::InstallOutcome;
use super::libraries::{download_library_entry, natives_os};
use super::{ManifestVersion, fetch_to_file};

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct VersionMeta {
    #[serde(default)]
    downloads: BTreeMap<String, DownloadInfo>,
    #[serde(default)]
    assetIndex: Option<AssetIndexInfo>,
    #[serde(default)]
    libraries: Vec<crate::game::version_json::LibraryEntry>,
}

#[derive(Debug, Deserialize)]
struct DownloadInfo {
    url: String,
    #[serde(default)]
    sha1: String,
}

#[derive(Debug, Deserialize)]
struct AssetIndexInfo {
    url: String,
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct AssetIndex {
    objects: BTreeMap<String, AssetObject>,
}

#[derive(Debug, Deserialize)]
struct AssetObject {
    hash: String,
}

/// Stream-download the client jar with byte progress, verify its SHA-1 and
/// place it atomically. The jar is the biggest single file of a version
/// install, so it is the only one worth a live byte fraction.
fn fetch_client_jar(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    sha1: &str,
    on_bytes: &mut dyn FnMut(u64, u64),
    lang: Language,
) -> Result<()> {
    let bytes = net::get_bytes_progress(agent, url, lang, &mut |done, total| {
        on_bytes(done, total)
    })?;
    super::verify_sha1(&bytes, sha1, &format!("{url} (downloaded)"), lang)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Install a Minecraft version into `game_dir` (vanilla layout:
/// `versions/<id>/<id>.{json,jar}`, `assets/indexes/<idx>.json`,
/// `assets/objects/<h[:2]>/<h>`).
///
/// Progress is `(label, fraction)`: the asset index is fetched first so the
/// total unit count (jar + libraries + asset objects) is known upfront and
/// the fraction is exact; the jar contributes its live byte fraction.
pub fn install_version(
    agent: &ureq::Agent,
    game_dir: &Path,
    version: &ManifestVersion,
    lang: Language,
    mut progress: &mut dyn FnMut(&str, Option<f32>),
) -> Result<InstallOutcome> {
    let id = &version.id;
    let version_dir = game_dir.join("versions").join(id);
    std::fs::create_dir_all(&version_dir)?;

    // 1. version.json
    let meta_bytes = net::get_bytes(agent, &version.url, lang)?;
    let meta: VersionMeta = serde_json::from_slice(&meta_bytes)
        .context(tr(lang, "failed to parse the version metadata"))?;
    let json_path = version_dir.join(format!("{id}.json"));
    std::fs::write(&json_path, &meta_bytes)?;

    // 2. asset index (fetched early: its object count is the denominator of
    //    the progress fraction)
    let asset_index = meta
        .assetIndex
        .context(tr(lang, "version metadata has no asset index"))?;
    let index_id = if asset_index.id.is_empty() {
        "legacy"
    } else {
        &asset_index.id
    };
    let index_path = game_dir
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));
    let index_bytes = net::get_bytes(agent, &asset_index.url, lang)?;
    std::fs::create_dir_all(index_path.parent().unwrap())?;
    std::fs::write(&index_path, &index_bytes)?;
    let index: AssetIndex = serde_json::from_slice(&index_bytes)
        .context(tr(lang, "failed to parse the asset index"))?;

    // Units: the client jar (driven by its own byte fraction) plus one unit
    // per library and per asset object.
    let total_units = 1.0f32 + meta.libraries.len() as f32 + index.objects.len() as f32;
    let frac = |done_units: f32| Some(done_units / total_units);
    let mut done_units = 0.0f32;

    // 3. client jar (streamed — the largest single download)
    let jar = meta.downloads.get("client").ok_or_else(|| {
        anyhow!(
            "{}",
            tr_fmt(lang, "version {0} has no client download", &[id])
        )
    })?;
    let jar_path = version_dir.join(format!("{id}.jar"));
    let mut downloaded = 0usize;
    {
        let dl_label = tr_fmt(lang, "Downloading {0}", &[id]);
        fetch_client_jar(
            agent,
            &jar.url,
            &jar_path,
            &jar.sha1,
            &mut |bytes_done: u64, bytes_total: u64| {
                let inner = if bytes_total > 0 {
                    bytes_done as f32 / bytes_total as f32
                } else {
                    0.0
                };
                progress(&dl_label, frac(done_units + inner));
            },
            lang,
        )?;
        done_units += 1.0;
        downloaded += 1;
    }

    // 4. libraries declared by the version json (current-OS natives jars
    //    included), so the first launch does not have to fetch them
    let libs_dir = game_dir.join("libraries");
    let natives = natives_os();
    let total = meta.libraries.len();
    for (i, lib) in meta.libraries.iter().enumerate() {
        download_library_entry(agent, &libs_dir, lib, natives, lang, &mut progress)?;
        done_units += 1.0;
        if (i + 1) % 10 == 0 || i + 1 == total {
            progress(
                &tr_fmt(
                    lang,
                    "libraries {0}/{1}",
                    &[&(i + 1).to_string(), &total.to_string()],
                ),
                frac(done_units),
            );
        }
    }

    // 5. asset objects
    let objects_dir = game_dir.join("assets").join("objects");
    let total = index.objects.len();
    progress(
        &tr_fmt(lang, "assets 0/{0}", &[&total.to_string()]),
        frac(done_units),
    );
    for (i, (name, object)) in index.objects.iter().enumerate() {
        let hash = object.hash.to_lowercase();
        let dest = objects_dir.join(&hash[..2]).join(&hash);
        let url = format!(
            "https://resources.download.minecraft.net/{}/{hash}",
            &hash[..2]
        );
        // Names are not URLs; use them only for progress display.
        let _ = name;
        if fetch_to_file(agent, &url, &dest, &hash, &mut progress, lang)? {
            downloaded += 1;
        }
        done_units += 1.0;
        if (i + 1) % 200 == 0 || i + 1 == total {
            progress(
                &tr_fmt(
                    lang,
                    "assets {0}/{1}",
                    &[&(i + 1).to_string(), &total.to_string()],
                ),
                frac(done_units),
            );
        }
    }

    Ok(InstallOutcome {
        version_id: id.clone(),
        downloaded_files: downloaded,
    })
}

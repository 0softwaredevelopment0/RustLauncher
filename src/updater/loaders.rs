//! Mod-loader build discovery: list the available Fabric, Quilt, NeoForge
//! and Forge builds for a Minecraft version from their meta APIs.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::lang::{tr, tr_fmt, Language};
use crate::util::net;

/// A mod loader that can be installed on top of a Minecraft version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Loader {
    Fabric,
    Quilt,
    NeoForge,
    Forge,
}

impl Loader {
    /// The identifier used in version ids and UI (`fabric-loader-0.19.5-1.21.4`).
    #[allow(dead_code)] // part of the public API for tests/future use
    pub fn slug(self) -> &'static str {
        match self {
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::NeoForge => "neoforge",
            Loader::Forge => "forge",
        }
    }

    /// UI label.
    pub fn label(self) -> &'static str {
        match self {
            Loader::Fabric => "Fabric",
            Loader::Quilt => "Quilt",
            Loader::NeoForge => "NeoForge",
            Loader::Forge => "Forge",
        }
    }

    /// All loaders in display order.
    pub const ALL: [Loader; 4] = [
        Loader::Fabric,
        Loader::Quilt,
        Loader::NeoForge,
        Loader::Forge,
    ];
}

/// One loader build available for a Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderBuild {
    /// The loader build version (`0.19.5`, `21.4.157`, `54.1.14`).
    pub version: String,
    /// Whether the build is a stable/recommended one (drives sorting).
    pub stable: bool,
}

/// List the loader builds available for `mc` (newest first, stable first).
pub fn fetch_loader_builds(
    agent: &ureq::Agent,
    loader: Loader,
    mc: &str,
    lang: Language,
) -> Result<Vec<LoaderBuild>> {
    let builds = match loader {
        Loader::Fabric => fetch_fabric_builds(agent, mc, lang)?,
        Loader::Quilt => fetch_quilt_builds(agent, mc, lang)?,
        Loader::NeoForge => fetch_neoforge_builds(agent, mc, lang)?,
        Loader::Forge => fetch_forge_builds(agent, mc, lang)?,
    };
    if builds.is_empty() {
        return Err(anyhow!(
            "{}",
            tr_fmt(lang, "no {0} builds found for {1}", &[loader.label(), mc])
        ));
    }
    Ok(builds)
}

#[derive(Debug, Deserialize)]
struct FabricLoaderEntry {
    loader: FabricLoaderInfo,
}

#[derive(Debug, Deserialize)]
struct FabricLoaderInfo {
    version: String,
    #[serde(default = "default_true")]
    stable: bool,
}

fn default_true() -> bool {
    true
}

fn fetch_fabric_builds(agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{mc}");
    let body = net::get_string(agent, &url, lang)?;
    let entries: Vec<FabricLoaderEntry> = serde_json::from_str(&body)
        .context(tr(lang, "failed to parse the Fabric meta response"))?;
    Ok(entries
        .into_iter()
        .map(|e| LoaderBuild {
            version: e.loader.version,
            stable: e.loader.stable,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct QuiltLoaderEntry {
    loader: QuiltLoaderInfo,
}

#[derive(Debug, Deserialize)]
struct QuiltLoaderInfo {
    version: String,
}

fn fetch_quilt_builds(agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.quiltmc.org/v3/versions/loader/{mc}");
    let body = net::get_string(agent, &url, lang)?;
    let entries: Vec<QuiltLoaderEntry> =
        serde_json::from_str(&body).context(tr(lang, "failed to parse the Quilt meta response"))?;
    Ok(entries
        .into_iter()
        .map(|e| {
            let stable = !e.loader.version.contains("beta");
            LoaderBuild {
                version: e.loader.version,
                stable,
            }
        })
        .collect())
}

fn fetch_neoforge_builds(
    _agent: &ureq::Agent,
    mc: &str,
    lang: Language,
) -> Result<Vec<LoaderBuild>> {
    let (major, minor) = neoforge_major_minor(mc, lang)?;
    let url = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
    let agent = net::agent();
    let body = net::get_string(&agent, url, lang)?;
    #[derive(Deserialize)]
    struct Versions {
        versions: Vec<String>,
    }
    let parsed: Versions = serde_json::from_str(&body)
        .context(tr(lang, "failed to parse the NeoForge maven metadata"))?;
    let prefix = format!("{major}.{minor}.");
    let mut builds: Vec<LoaderBuild> = parsed
        .versions
        .into_iter()
        .filter(|v| v.starts_with(&prefix))
        .map(|v| {
            // NeoForge 26.x ships through `-beta`/`-alpha` suffixed builds;
            // a bare version string is a stable release.
            let stable = !v.contains("-beta") && !v.contains("-alpha");
            LoaderBuild { version: v, stable }
        })
        .collect();
    // Newest build first.
    builds.sort_by(|a, b| cmp_version_parts(&a.version, &b.version).reverse());
    Ok(builds)
}

/// NeoForge is numbered after the MC minor: MC `1.21.4` -> NeoForge
/// `21.4.<build>`, and year-based ids map directly: MC `26.3` ->
/// `26.3.<build>`. Older MC versions have no NeoForge; returns an error
/// the UI can show for those.
fn neoforge_major_minor(mc: &str, lang: Language) -> Result<(u32, u32)> {
    let mut parts = mc.split('.');
    let first = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
            )
        })?;
    let second = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
            )
        })?;
    // Legacy `1.x.y`: NeoForge numbers are `x.y.<build>` (leading 1
    // dropped) and require 1.20.2+. Year-based ids (`26.3`) map directly
    // to `26.3.<build>`.
    if first == 1 {
        let patch = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
                )
            })?;
        if second < 20 || (second == 20 && patch < 2) {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    lang,
                    "NeoForge does not support {0} (requires 1.20.2+)",
                    &[mc]
                )
            ));
        }
        Ok((second, patch))
    } else {
        Ok((first, second))
    }
}

#[derive(Debug, Deserialize)]
struct ForgePromotions {
    #[serde(default)]
    promos: BTreeMap<String, String>,
}

fn fetch_forge_builds(_agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
    let agent = net::agent();
    let body = net::get_string(&agent, url, lang)?;
    let promos: ForgePromotions =
        serde_json::from_str(&body).context(tr(lang, "failed to parse the Forge promotions"))?;
    let mut builds: Vec<LoaderBuild> = Vec::new();
    for key in ["recommended", "latest"] {
        let promo_key = format!("{mc}-{key}");
        if let Some(v) = promos.promos.get(&promo_key) {
            if !builds.iter().any(|b| &b.version == v) {
                builds.push(LoaderBuild {
                    version: v.clone(),
                    stable: true,
                });
            }
        }
    }
    Ok(builds)
}

/// Numeric-aware comparison of dot-separated versions; each segment
/// compares by its leading digits (`54.1.9` < `54.1.14`, `37-beta` -> 37).
fn cmp_version_parts(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u64> = a.split('.').map(numeric_prefix).collect();
    let pb: Vec<u64> = b.split('.').map(numeric_prefix).collect();
    pa.cmp(&pb)
}

/// The leading digit run of a version segment (`37-beta` -> 37, no digits -> 0).
fn numeric_prefix(segment: &str) -> u64 {
    let digits: String = segment.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fabric_meta_parses() {
        let body = r#"[
            {"loader": {"version": "0.19.5", "stable": true},
             "intermediary": {"version": "1.21.4"}},
            {"loader": {"version": "0.20.0", "stable": false}}
        ]"#;
        let entries: Vec<FabricLoaderEntry> = serde_json::from_str(body).unwrap();
        let builds: Vec<LoaderBuild> = entries
            .into_iter()
            .map(|e| LoaderBuild {
                version: e.loader.version,
                stable: e.loader.stable,
            })
            .collect();
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].version, "0.19.5");
        assert!(builds[0].stable);
        assert!(!builds[1].stable);
    }

    #[test]
    fn quilt_meta_parses() {
        let body = r#"[
            {"loader": {"maven": "org.quiltmc:quilt-loader:0.20.0-beta.9",
                        "version": "0.20.0-beta.9", "build": 9}},
            {"loader": {"maven": "org.quiltmc:quilt-loader:0.21.0",
                        "version": "0.21.0", "build": 3}}
        ]"#;
        let entries: Vec<QuiltLoaderEntry> = serde_json::from_str(body).unwrap();
        let builds: Vec<LoaderBuild> = entries
            .into_iter()
            .map(|e| {
                let stable = !e.loader.version.contains("beta");
                LoaderBuild {
                    version: e.loader.version,
                    stable,
                }
            })
            .collect();
        assert_eq!(builds.len(), 2);
        assert!(!builds[0].stable);
        assert!(builds[1].stable);
    }

    #[test]
    fn forge_promotions_parse_and_filter() {
        let body = r#"{
            "homepage": "https://files.minecraftforge.net",
            "promos": {
                "1.21.4-latest": "54.1.18",
                "1.21.4-recommended": "54.1.14",
                "1.20.1-latest": "47.4.23"
            }
        }"#;
        let promos: ForgePromotions = serde_json::from_str(body).unwrap();
        assert_eq!(promos.promos.get("1.21.4-latest").unwrap(), "54.1.18");
        assert_eq!(promos.promos.get("1.20.1-latest").unwrap(), "47.4.23");
        assert!(!promos.promos.contains_key("9.9.9-latest"));
    }

    #[test]
    fn version_parts_compare_numerically() {
        use std::cmp::Ordering;
        assert_eq!(cmp_version_parts("54.1.9", "54.1.14"), Ordering::Less);
        assert_eq!(cmp_version_parts("54.1.14", "54.1.9"), Ordering::Greater);
        assert_eq!(cmp_version_parts("21.4.157", "21.4.157"), Ordering::Equal);
    }

    #[test]
    fn neoforge_version_mapping_rejects_old_mc() {
        assert!(neoforge_major_minor("1.21.4", crate::lang::Language::English).is_ok());
        assert!(neoforge_major_minor("1.20.2", crate::lang::Language::English).is_ok());
        assert!(neoforge_major_minor("1.20.1", crate::lang::Language::English).is_err());
        assert!(neoforge_major_minor("1.12.2", crate::lang::Language::English).is_err());
        assert!(neoforge_major_minor("not-a-version", crate::lang::Language::English).is_err());
        // Year-based ids map directly to `<year>.<minor>.<build>`.
        assert_eq!(
            neoforge_major_minor("26.3", crate::lang::Language::English).unwrap(),
            (26, 3)
        );
        assert_eq!(
            neoforge_major_minor("1.21.4", crate::lang::Language::English).unwrap(),
            (21, 4)
        );
    }
}

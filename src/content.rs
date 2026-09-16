//! Mod platform integration: search and download mods, resource packs,
//! shaders and worlds from Modrinth (Labrinth API v2) and CurseForge
//! (CFCore API v1, requires a user-supplied API key).
//!
//! Downloads go straight into the game directory the same way the vanilla
//! launcher lays out files: `mods/`, `resourcepacks/`, `shaderpacks/` and
//! `saves/<name>/` for worlds (world zips are extracted).

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::net;

/// The kind of downloadable content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContentKind {
    #[default]
    Mod,
    ResourcePack,
    Shader,
    World,
}

impl ContentKind {
    pub fn label(self) -> &'static str {
        match self {
            ContentKind::Mod => "Mods",
            ContentKind::ResourcePack => "Resource Packs",
            ContentKind::Shader => "Shaders",
            ContentKind::World => "Worlds",
        }
    }

    /// The Modrinth `project_type` facet value.
    pub fn modrinth_type(self) -> &'static str {
        match self {
            ContentKind::Mod => "mod",
            ContentKind::ResourcePack => "resourcepack",
            ContentKind::Shader => "shader",
            ContentKind::World => "world",
        }
    }

    /// The CurseForge `classId` for Minecraft content.
    pub fn curseforge_class(self) -> u32 {
        match self {
            ContentKind::Mod => 6,           // Mods
            ContentKind::ResourcePack => 12, // Resource Packs
            ContentKind::Shader => 6555,     // Shaders
            ContentKind::World => 17,        // World Gen / saves live under 17? Maps use 4471
        }
    }

    /// The destination directory inside the game dir.
    pub fn dest_dir(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::ResourcePack => "resourcepacks",
            ContentKind::Shader => "shaderpacks",
            ContentKind::World => "saves",
        }
    }
}

/// One search result (platform-agnostic).
#[derive(Debug, Clone)]
pub struct ContentItem {
    /// Platform slug/id used to fetch versions.
    pub id: String,
    pub title: String,
    pub author: String,
    pub description: String,
    /// Downloads count, for display.
    pub downloads: u64,
    /// Icon URL (may be empty; not rendered yet, kept for the UI).
    #[allow(dead_code)]
    pub icon_url: String,
}

/// One downloadable version file of a content project.
#[derive(Debug, Clone)]
pub struct ContentFile {
    /// Display name (`19.36.1.16 for Fabric 1.21.4`).
    pub name: String,
    /// Download URL of the primary file.
    pub url: String,
    /// File name to save as.
    pub file_name: String,
    pub size: u64,
    pub game_versions: Vec<String>,
    /// Loader names among the game versions (CurseForge only; empty for
    /// Modrinth where loaders are a separate field).
    #[allow(dead_code)]
    pub loaders: Vec<String>,
}

// ---------------------------------------------------------------------------
// Modrinth (Labrinth API v2, no key required)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ModrinthSearch {
    hits: Vec<ModrinthHit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ModrinthHit {
    project_id: String,
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    icon_url: String,
}

#[derive(Debug, Deserialize)]
struct ModrinthVersion {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    files: Vec<ModrinthFile>,
}

#[derive(Debug, Deserialize)]
struct ModrinthFile {
    url: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    primary: bool,
}

/// Search Modrinth for content.
pub fn search_modrinth(
    agent: &ureq::Agent,
    kind: ContentKind,
    query: &str,
    mc: &str,
    loader: Option<&str>,
    limit: usize,
) -> Result<Vec<ContentItem>> {
    let mut url = format!(
        "https://api.modrinth.com/v2/search?limit={limit}&index=downloads&facets=[[%22project_type:{}%22]]",
        kind.modrinth_type()
    );
    if !query.trim().is_empty() {
        url.push_str(&format!("&query={}", urlquery(query.trim())));
    }
    if !mc.is_empty() {
        url.push_str(&format!(",[[%22versions:{}%22]]", urlquery(mc)));
    }
    if let Some(loader) = loader {
        // Only mods are loader-specific on Modrinth.
        if kind == ContentKind::Mod {
            url.push_str(&format!(",[[%22categories:{}%22]]", urlquery(loader)));
        }
    }
    let body = net::get_string(agent, &url)?;
    let parsed: ModrinthSearch =
        serde_json::from_str(&body).context("failed to parse the Modrinth search response")?;
    Ok(parsed
        .hits
        .into_iter()
        .map(|h| ContentItem {
            id: h.project_id,
            title: h.title,
            author: h.author,
            description: h.description,
            downloads: h.downloads,
            icon_url: h.icon_url,
        })
        .collect())
}

/// List the downloadable files of a Modrinth project, newest first.
pub fn modrinth_versions(
    agent: &ureq::Agent,
    project_id: &str,
    mc: &str,
    loader: Option<&str>,
) -> Result<Vec<ContentFile>> {
    let mut url = format!("https://api.modrinth.com/v2/project/{project_id}/version");
    if !mc.is_empty() {
        url.push_str(&format!("?game_versions=[%22{}%22]", urlquery(mc)));
        if let Some(loader) = loader {
            url.push_str(&format!("&loaders=[%22{}%22]", urlquery(loader)));
        }
    }
    let body = net::get_string(agent, &url)?;
    let parsed: Vec<ModrinthVersion> =
        serde_json::from_str(&body).context("failed to parse the Modrinth versions response")?;
    Ok(parsed
        .into_iter()
        .map(|v| {
            // Prefer the primary file, else the first one.
            let file = v
                .files
                .iter()
                .find(|f| f.primary)
                .or_else(|| v.files.first());
            let (url, filename, size) = match file {
                Some(f) => (f.url.clone(), f.filename.clone(), f.size),
                None => (String::new(), String::new(), 0),
            };
            let name = if v.name.is_empty() {
                v.version_number
            } else {
                v.name
            };
            ContentFile {
                name,
                url,
                file_name: filename,
                size,
                game_versions: v.game_versions,
                loaders: v.loaders,
            }
        })
        .filter(|f| !f.url.is_empty())
        .collect())
}

// ---------------------------------------------------------------------------
// CurseForge (CFCore API v1, requires an API key)
// ---------------------------------------------------------------------------

/// Search CurseForge. `api_key` is the x-api-key value the user configured.
pub fn search_curseforge(
    agent: &ureq::Agent,
    api_key: &str,
    kind: ContentKind,
    query: &str,
    mc: &str,
    limit: usize,
) -> Result<Vec<ContentItem>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!(
            "CurseForge needs an API key — set it in Settings (cfwidget fallback unavailable)"
        ));
    }
    // The class id selects the content category (432 is Minecraft).
    let class_id = kind.curseforge_class();
    let mut url = format!(
        "https://api.curseforge.com/v1/mods/search?gameId=432&classId={class_id}&pageSize={limit}&sortField=2&sortOrder=desc"
    );
    if !query.trim().is_empty() {
        url.push_str(&format!("&searchFilter={}", urlquery(query.trim())));
    }
    if !mc.is_empty() {
        url.push_str(&format!("&gameVersion={}", urlquery(mc)));
    }
    let body = agent
        .get(&url)
        .set("x-api-key", api_key.trim())
        .set("Accept", "application/json")
        .call()
        .map_err(net::classify)?;
    if body.status() != 200 {
        return Err(anyhow!("HTTP {} from CurseForge", body.status()));
    }
    let text = body
        .into_string()
        .context("failed to read the CurseForge response")?;
    let parsed: CurseSearch =
        serde_json::from_str(&text).context("failed to parse the CurseForge search response")?;
    Ok(parsed
        .data
        .into_iter()
        .map(|m| ContentItem {
            id: m.id.to_string(),
            title: m.name,
            author: m
                .authors
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            description: m.summary,
            downloads: m.download_count,
            icon_url: m.logo.map(|l| l.thumbnail_url).unwrap_or_default(),
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct CurseSearch {
    #[serde(default)]
    data: Vec<CurseMod>,
}

#[derive(Debug, Deserialize)]
struct CurseMod {
    id: u64,
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    authors: Vec<CurseAuthor>,
    #[serde(default)]
    logo: Option<CurseLogo>,
}

#[derive(Debug, Deserialize)]
struct CurseAuthor {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CurseLogo {
    #[serde(default, rename = "thumbnailUrl")]
    thumbnail_url: String,
}

/// List the files of a CurseForge project, newest first.
pub fn curseforge_files(
    agent: &ureq::Agent,
    api_key: &str,
    project_id: &str,
    mc: &str,
) -> Result<Vec<ContentFile>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("CurseForge needs an API key — set it in Settings"));
    }
    let body = agent
        .get(&format!(
            "https://api.curseforge.com/v1/mods/{project_id}/files"
        ))
        .set("x-api-key", api_key.trim())
        .set("Accept", "application/json")
        .call()
        .map_err(net::classify)?;
    if body.status() != 200 {
        return Err(anyhow!("HTTP {} from CurseForge", body.status()));
    }
    let text = body
        .into_string()
        .context("failed to read the CurseForge files response")?;
    let parsed: CurseFiles =
        serde_json::from_str(&text).context("failed to parse the CurseForge files response")?;
    let mut files: Vec<ContentFile> = parsed
        .data
        .into_iter()
        .map(|f| {
            let loaders = f
                .game_versions
                .iter()
                .filter(|v| is_loader_name(v))
                .cloned()
                .collect();
            ContentFile {
                name: f.display_name,
                url: f.download_url.unwrap_or_default(),
                file_name: f.file_name,
                size: f.file_length,
                game_versions: f.game_versions,
                loaders,
            }
        })
        .filter(|f| !f.url.is_empty())
        .collect();
    // Newest first (server order is not guaranteed).
    files.truncate(200);
    if !mc.is_empty() {
        files.retain(|f| f.game_versions.iter().any(|v| v == mc) || f.game_versions.is_empty());
    }
    Ok(files)
}

#[derive(Debug, Deserialize)]
struct CurseFiles {
    #[serde(default)]
    data: Vec<CurseFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseFile {
    display_name: String,
    file_name: String,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    file_length: u64,
    #[serde(default)]
    game_versions: Vec<String>,
}

/// `fabric`/`forge`/… as opposed to a game version like `1.20.1`.
fn is_loader_name(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "fabric" | "forge" | "neoforge" | "quilt" | "rift" | "liteloader" | "modloader"
    )
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Percent-encode the query-component characters that break URLs.
fn urlquery(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Download a content file into `game_dir/<dest>/[<subdir>]`. Returns the
/// path of the saved file. World zips are extracted into
/// `saves/<zip-stem>/` so the world appears directly in the world list.
pub fn download_file(
    agent: &ureq::Agent,
    game_dir: &Path,
    kind: ContentKind,
    file: &ContentFile,
) -> Result<std::path::PathBuf> {
    if file.url.is_empty() {
        return Err(anyhow!("the file has no download URL"));
    }
    let bytes = net::get_bytes(agent, &file.url)?;
    let file_name = if file.file_name.is_empty() {
        // Derive from the URL tail.
        file.url
            .rsplit('/')
            .next()
            .unwrap_or("download.bin")
            .to_string()
    } else {
        file.file_name.clone()
    };

    let dir = game_dir.join(kind.dest_dir());
    std::fs::create_dir_all(&dir)?;

    if kind == ContentKind::World && file_name.to_ascii_lowercase().ends_with(".zip") {
        // Extract the world zip into saves/<stem>/.
        let stem = file_name.trim_end_matches(".zip").trim_end_matches(".ZIP");
        let world_dir = dir.join(stem);
        std::fs::create_dir_all(&world_dir)?;
        extract_zip(&bytes, &world_dir)?;
        return Ok(world_dir);
    }

    let dest = dir.join(&file_name);
    std::fs::write(&dest, &bytes).with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(dest)
}

/// Extract a zip archive into `dest` (used for world saves).
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<usize> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).context("failed to open the zip archive")?;
    let mut count = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("bad zip entry {i}"))?;
        let Some(name) = entry.enclosed_name() else {
            continue; // skip unsafe paths
        };
        let out_path = dest.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            count += 1;
        }
    }
    if count == 0 {
        return Err(anyhow!("the archive contained no files"));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlquery_encodes_specials() {
        assert_eq!(urlquery("jei"), "jei");
        assert_eq!(urlquery("sodium extra"), "sodium%20extra");
        assert_eq!(urlquery("a+b&c"), "a%2Bb%26c");
        assert_eq!(urlquery("1.20.1"), "1.20.1");
    }

    #[test]
    fn modrinth_search_url_shapes() {
        // The search function builds its URL inline; verify the facet values.
        assert_eq!(ContentKind::Mod.modrinth_type(), "mod");
        assert_eq!(ContentKind::ResourcePack.modrinth_type(), "resourcepack");
        assert_eq!(ContentKind::Shader.modrinth_type(), "shader");
        assert_eq!(ContentKind::World.modrinth_type(), "world");
    }

    #[test]
    fn dest_dirs_match_vanilla_layout() {
        assert_eq!(ContentKind::Mod.dest_dir(), "mods");
        assert_eq!(ContentKind::ResourcePack.dest_dir(), "resourcepacks");
        assert_eq!(ContentKind::Shader.dest_dir(), "shaderpacks");
        assert_eq!(ContentKind::World.dest_dir(), "saves");
    }

    #[test]
    fn modrinth_search_parses() {
        let body = r#"{"hits":[{"project_id":"u6dRKJwZ","project_type":"mod","title":"JEI","author":"mezz","description":"d","downloads":1000,"icon_url":""}]}"#;
        let parsed: ModrinthSearch = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.hits.len(), 1);
        assert_eq!(parsed.hits[0].project_id, "u6dRKJwZ");
        assert_eq!(parsed.hits[0].downloads, 1000);
    }

    #[test]
    fn modrinth_versions_parse_and_prefer_primary_file() {
        let body = r#"[
            {"name":"JEI 1.21.4","version_number":"19.21.4","game_versions":["1.21.4"],
             "loaders":["fabric","quilt"],
             "files":[
                {"url":"https://cdn/x.jar","filename":"secondary.jar","size":1,"primary":false},
                {"url":"https://cdn/y.jar","filename":"primary.jar","size":2,"primary":true}
             ]}
        ]"#;
        let parsed: Vec<ModrinthVersion> = serde_json::from_str(body).unwrap();
        let files: Vec<ContentFile> = parsed
            .into_iter()
            .map(|v| {
                let file = v.files.iter().find(|f| f.primary).unwrap();
                ContentFile {
                    name: v.name,
                    url: file.url.clone(),
                    file_name: file.filename.clone(),
                    size: file.size,
                    game_versions: v.game_versions,
                    loaders: v.loaders,
                }
            })
            .collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name, "primary.jar");
        assert_eq!(files[0].loaders.len(), 2);
    }

    #[test]
    fn curseforge_responses_parse() {
        let search = r#"{"data":[{"id":238222,"name":"JEI","summary":"s","download_count":5,
            "authors":[{"name":"mezz"}],"logo":{"thumbnailUrl":"http://x/t.png"}}]}"#;
        let parsed: CurseSearch = serde_json::from_str(search).unwrap();
        assert_eq!(parsed.data[0].id, 238222);
        assert_eq!(parsed.data[0].authors[0].name, "mezz");

        let files = r#"{"data":[{"displayName":"JEI 1.20.1","fileName":"jei.jar",
            "downloadUrl":"https://edge/jei.jar","fileLength":123,
            "gameVersions":["1.20.1","Forge"]}]}"#;
        let parsed: CurseFiles = serde_json::from_str(files).unwrap();
        assert_eq!(parsed.data[0].file_name, "jei.jar");
        assert_eq!(
            parsed.data[0].download_url.as_deref(),
            Some("https://edge/jei.jar")
        );
    }

    #[test]
    fn loader_names_vs_game_versions() {
        assert!(is_loader_name("Forge"));
        assert!(is_loader_name("fabric"));
        assert!(!is_loader_name("1.20.1"));
    }

    #[test]
    fn world_zip_extracts_into_dir() {
        // Build a tiny zip in memory with the `zip` crate.
        let buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buf);
        writer
            .start_file("world/level.dat", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"data").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let tmp = std::env::temp_dir().join(format!("rl-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let n = extract_zip(&bytes, &tmp).unwrap();
        assert_eq!(n, 1);
        assert!(tmp.join("world/level.dat").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

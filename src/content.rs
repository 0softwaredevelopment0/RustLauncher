//! Mod platform integration: search and download mods, resource packs,
//! shaders and worlds from Modrinth (Labrinth API v2).
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
    DataPack,
    Shader,
    Plugin,
    Modpack,
    Server,
    World,
}

impl ContentKind {
    pub fn label(self) -> &'static str {
        match self {
            ContentKind::Mod => "Mods",
            ContentKind::ResourcePack => "Resource Packs",
            ContentKind::DataPack => "Data Packs",
            ContentKind::Shader => "Shaders",
            ContentKind::Plugin => "Plugins",
            ContentKind::Modpack => "Modpacks",
            ContentKind::Server => "Servers",
            ContentKind::World => "Worlds",
        }
    }

    /// The Modrinth `project_type` facet value.
    pub fn modrinth_type(self) -> &'static str {
        match self {
            ContentKind::Mod => "mod",
            ContentKind::ResourcePack => "resourcepack",
            ContentKind::DataPack => "datapack",
            ContentKind::Shader => "shader",
            ContentKind::Plugin => "plugin",
            ContentKind::Modpack => "modpack",
            // Modrinth has no server project type; browse server software
            // through the `plugin` type filtered to server loaders instead.
            ContentKind::Server => "plugin",
            ContentKind::World => "world",
        }
    }

    /// Whether downloads of this kind go to the user's Downloads folder
    /// instead of the game directory (server-side things, packs whose
    /// in-game folders are managed manually).
    pub fn goes_to_downloads(self) -> bool {
        matches!(
            self,
            ContentKind::DataPack | ContentKind::Shader | ContentKind::Plugin | ContentKind::Server
        )
    }

    /// The destination directory inside the game dir (non-Downloads kinds).
    pub fn dest_dir(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::ResourcePack => "resourcepacks",
            ContentKind::World => "saves",
            // Modpacks unpack into their own versions/<id> dir at install.
            ContentKind::Modpack => "versions",
            _ => "downloads",
        }
    }
}

/// Modrinth search sort orders (the five the site offers).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortIndex {
    #[default]
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

impl SortIndex {
    pub fn label(self) -> &'static str {
        match self {
            SortIndex::Relevance => "Relevance",
            SortIndex::Downloads => "Downloads",
            SortIndex::Follows => "Follows",
            SortIndex::Newest => "Newest",
            SortIndex::Updated => "Updated",
        }
    }

    /// The `index=` query value.
    pub fn param(self) -> &'static str {
        match self {
            SortIndex::Relevance => "relevance",
            SortIndex::Downloads => "downloads",
            SortIndex::Follows => "follows",
            SortIndex::Newest => "newest",
            SortIndex::Updated => "updated",
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
    /// Display categories (loaders + tags), lowercase.
    pub categories: Vec<String>,
    /// License short name (`MIT`, `LicenseRef-All-Rights-Reserved`, …).
    #[allow(dead_code)] // displayed later in the project card
    pub license: String,
    /// Follows count (Modrinth).
    pub follows: u64,
    /// Date of the latest update (ISO-8601, may be empty).
    #[allow(dead_code)] // displayed later in the project card
    pub date_updated: String,
}

/// The long description of a project (Modrinth `body`, markdown).
#[derive(Debug, Clone, Default)]
pub struct ProjectDetail {
    #[allow(dead_code)] // the title is in the card header
    pub title: String,
    #[allow(dead_code)]
    pub description: String,
    /// The full markdown body of the project page.
    pub body: String,
    #[allow(dead_code)]
    pub license: String,
    #[allow(dead_code)]
    pub downloads: u64,
    #[allow(dead_code)]
    pub follows: u64,
    #[allow(dead_code)]
    pub icon_url: String,
    #[allow(dead_code)]
    pub categories: Vec<String>,
    pub game_versions: Vec<String>,
    pub date_updated: String,
    /// Issue tracker / source links.
    pub issues_url: String,
    pub source_url: String,
    pub wiki_url: String,
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
    /// Game versions the file supports (shown as a hint in the file list).
    #[allow(dead_code)] // display-only metadata
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
    follows: u64,
    #[serde(default)]
    icon_url: String,
    #[serde(default)]
    display_categories: Vec<String>,
    #[serde(default)]
    license: String,
    #[serde(default)]
    date_modified: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ModrinthProject {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    followers: u64,
    #[serde(default)]
    icon_url: String,
    #[serde(default)]
    display_categories: Vec<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    issues_url: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    wiki_url: Option<String>,
    #[serde(default)]
    license: ModrinthLicense,
}

#[derive(Debug, Default, Deserialize)]
struct ModrinthLicense {
    #[serde(default)]
    id: String,
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
#[allow(clippy::too_many_arguments)]
pub fn search_modrinth(
    agent: &ureq::Agent,
    kind: ContentKind,
    query: &str,
    mc: &str,
    loader: Option<&str>,
    categories: &[String],
    license: Option<&str>,
    sort: SortIndex,
    limit: usize,
) -> Result<Vec<ContentItem>> {
    // One facet group per dimension; values inside a group are OR, groups
    // are AND (documented Labrinth behavior).
    let mut facets: Vec<String> = vec![format!("[\"project_type:{}\"]", kind.modrinth_type())];
    if !mc.trim().is_empty() {
        facets.push(format!("[\"versions:{}\"]", urlquery(mc.trim())));
    }
    if let Some(loader) = loader {
        if kind == ContentKind::Mod || kind == ContentKind::Plugin {
            facets.push(format!("[\"categories:{}\"]", urlquery(loader)));
        }
    }
    if !categories.is_empty() {
        let inner: Vec<String> = categories
            .iter()
            .map(|c| format!("\"categories:{}\"", urlquery(c)))
            .collect();
        facets.push(format!("[{}]", inner.join(",")));
    }
    if let Some(license) = license {
        if !license.trim().is_empty() {
            facets.push(format!("[\"license:{}\"]", urlquery(license.trim())));
        }
    }
    let mut url = format!(
        "https://api.modrinth.com/v2/search?limit={limit}&index={}&facets=%5B{}%5D",
        sort.param(),
        urlquery(&facets.join(","))
    );
    if !query.trim().is_empty() {
        url.push_str(&format!("&query={}", urlquery(query.trim())));
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
            categories: h.display_categories,
            license: h.license,
            follows: h.follows,
            date_updated: h.date_modified,
        })
        .collect())
}

/// Fetch the full project page of a Modrinth project.
pub fn modrinth_project(agent: &ureq::Agent, project_id: &str) -> Result<ProjectDetail> {
    let body = net::get_string(
        agent,
        &format!("https://api.modrinth.com/v2/project/{project_id}"),
    )?;
    let parsed: ModrinthProject =
        serde_json::from_str(&body).context("failed to parse the Modrinth project")?;
    Ok(ProjectDetail {
        title: parsed.title,
        description: parsed.description,
        body: parsed.body,
        license: parsed.license.id,
        downloads: parsed.downloads,
        follows: parsed.followers,
        icon_url: parsed.icon_url,
        categories: parsed.display_categories,
        game_versions: parsed.game_versions,
        date_updated: parsed.updated,
        issues_url: parsed.issues_url.unwrap_or_default(),
        source_url: parsed.source_url.unwrap_or_default(),
        wiki_url: parsed.wiki_url.unwrap_or_default(),
    })
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
        assert_eq!(ContentKind::World.dest_dir(), "saves");
        // Things without an in-game folder go to the user's Downloads.
        assert!(ContentKind::DataPack.goes_to_downloads());
        assert!(ContentKind::Shader.goes_to_downloads());
        assert!(ContentKind::Plugin.goes_to_downloads());
        assert!(ContentKind::Server.goes_to_downloads());
        assert!(!ContentKind::Mod.goes_to_downloads());
        assert!(!ContentKind::ResourcePack.goes_to_downloads());
    }

    #[test]
    fn sort_index_covers_all_five_modrinth_orders() {
        assert_eq!(SortIndex::default(), SortIndex::Relevance);
        let params: Vec<&str> = [
            SortIndex::Relevance,
            SortIndex::Downloads,
            SortIndex::Follows,
            SortIndex::Newest,
            SortIndex::Updated,
        ]
        .iter()
        .map(|s| s.param())
        .collect();
        assert_eq!(
            params,
            vec!["relevance", "downloads", "follows", "newest", "updated"]
        );
    }

    #[test]
    fn modrinth_project_parses() {
        let body = r#"{
            "title": "Sodium",
            "description": "fast",
            "body": "Text with [link](http://x) inside.",
            "downloads": 100,
            "followers": 10,
            "icon_url": "",
            "display_categories": ["fabric", "optimization"],
            "game_versions": ["1.21.4"],
            "updated": "2026-01-01T00:00:00Z",
            "issues_url": "http://issues",
            "source_url": null,
            "wiki_url": null,
            "license": {"id": "LicenseRef-Polyform-Shield-1.0.0"}
        }"#;
        let parsed: ModrinthProject = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.title, "Sodium");
        assert_eq!(parsed.license.id, "LicenseRef-Polyform-Shield-1.0.0");
        assert_eq!(parsed.game_versions.len(), 1);
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

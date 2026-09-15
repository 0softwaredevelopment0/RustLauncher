//! Parsing of Minecraft `version.json` files.
//!
//! The Java launcher located JSON fields with hand-rolled string searches
//! (`content.indexOf("\"mainClass\"")`), which matched keys inside string
//! values or unrelated objects and broke on reordering. Here the JSON is
//! parsed properly with serde and all fields are read structurally.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// The subset of version.json RustLauncher needs to launch the game.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    /// Vanilla main class fallback for old formats.
    #[serde(default)]
    pub main_class: Option<String>,

    /// The underlying vanilla version (Fabric/Forge reference it).
    #[serde(default)]
    pub jar: Option<String>,

    /// Asset index id for older formats (`"assets": "5"`).
    #[serde(default)]
    pub assets: Option<String>,

    /// Asset index id for newer formats (`"assetIndex": {"id": "5"}`).
    #[serde(default)]
    pub asset_index: Option<AssetIndex>,

    /// Required Java major version.
    #[serde(default)]
    pub java_version: Option<JavaVersion>,

    /// Library list (Maven coordinates).
    #[serde(default)]
    pub libraries: Vec<LibraryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    #[serde(default)]
    pub major_version: Option<u32>,
    /// Runtime component name (e.g. "java-runtime-gamma") — kept for
    /// diagnostics, not used for launch decisions.
    #[serde(default)]
    #[allow(dead_code)]
    pub component: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
}

impl VersionJson {
    /// Load and parse a version.json file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    /// The main class: from the JSON, else the vanilla fallback.
    pub fn main_class(&self) -> &str {
        self.main_class
            .as_deref()
            .unwrap_or("net.minecraft.client.main.Main")
    }

    /// The asset index id: `assetIndex.id`, else `assets`, else the version name.
    pub fn asset_index_id<'a>(&'a self, version_name: &'a str) -> &'a str {
        if let Some(idx) = &self.asset_index {
            &idx.id
        } else if let Some(assets) = &self.assets {
            assets
        } else {
            version_name
        }
    }

    /// Required Java major version, if declared.
    pub fn required_java_major(&self) -> Option<u32> {
        self.java_version.as_ref().and_then(|j| j.major_version)
    }

    /// Whether the version references LWJGL 3 (affects classpath conflicts).
    pub fn uses_lwjgl3(&self) -> bool {
        self.libraries.iter().any(|lib| {
            lib.name.starts_with("org.lwjgl:lwjgl-glfw")
                || lib.name.starts_with("org.lwjgl:lwjgl:3.")
        })
    }

    /// Resolve the `jar` reference (vanilla version the modloader builds upon).
    pub fn jar_reference(&self) -> Option<&str> {
        self.jar.as_deref()
    }
}

/// Resolve a Maven coordinate (`group:artifact:version[:classifier]`) to a
/// jar path under the libraries directory.
pub fn maven_coordinate_to_path(
    libraries_dir: &Path,
    coordinate: &str,
) -> Option<std::path::PathBuf> {
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group_path = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let mut file_name = format!("{artifact}-{version}");
    if let Some(classifier) = parts.get(3) {
        if !classifier.is_empty() {
            file_name.push('-');
            file_name.push_str(classifier);
        }
    }
    file_name.push_str(".jar");
    let path = libraries_dir
        .join(group_path)
        .join(artifact)
        .join(version)
        .join(file_name);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_vanilla_style_json() {
        let tmp = std::env::temp_dir().join(format!("rl-json-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("1.20.1.json");
        fs::write(
            &path,
            r#"{
                "id": "1.20.1",
                "mainClass": "net.minecraft.client.main.Main",
                "assets": "5",
                "javaVersion": {"component": "java-runtime-gamma", "majorVersion": 17},
                "libraries": [{"name": "com.google.gson:gson:2.10.1"}]
            }"#,
        )
        .unwrap();

        let json = VersionJson::load(&path).unwrap();
        assert_eq!(json.main_class(), "net.minecraft.client.main.Main");
        assert_eq!(json.asset_index_id("1.20.1"), "5");
        assert_eq!(json.required_java_major(), Some(17));
        assert!(!json.uses_lwjgl3());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parses_fabric_style_json_with_asset_index_object() {
        let tmp = std::env::temp_dir().join(format!("rl-json-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("fabric.json");
        fs::write(
            &path,
            r#"{
                "id": "fabric-1.20.1",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "jar": "1.20.1",
                "assetIndex": {"id": "5", "totalSize": 100, "url": "https://example"},
                "javaVersion": {"majorVersion": 17},
                "libraries": [
                    {"name": "net.fabricmc:fabric-loader:0.15.0"},
                    {"name": "org.lwjgl:lwjgl-glfw:3.3.2"}
                ]
            }"#,
        )
        .unwrap();

        let json = VersionJson::load(&path).unwrap();
        assert_eq!(
            json.main_class(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(json.jar_reference(), Some("1.20.1"));
        assert_eq!(json.asset_index_id("fabric-1.20.1"), "5");
        assert_eq!(json.required_java_major(), Some(17));
        assert!(json.uses_lwjgl3());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn main_class_falls_back_to_vanilla() {
        let json: VersionJson = serde_json::from_str("{}").unwrap();
        assert_eq!(json.main_class(), "net.minecraft.client.main.Main");
        assert_eq!(json.asset_index_id("1.7.10"), "1.7.10");
        assert_eq!(json.required_java_major(), None);
    }

    #[test]
    fn maven_coordinates_resolve_to_existing_jars() {
        let tmp = std::env::temp_dir().join(format!("rl-maven-{}", std::process::id()));
        let libs = tmp.join("libraries");
        let jar = libs.join("net/fabricmc/fabric-loader/0.15.0/fabric-loader-0.15.0.jar");
        fs::create_dir_all(jar.parent().unwrap()).unwrap();
        fs::write(&jar, b"PK").unwrap();

        assert_eq!(
            maven_coordinate_to_path(&libs, "net.fabricmc:fabric-loader:0.15.0"),
            Some(jar)
        );
        assert_eq!(
            maven_coordinate_to_path(&libs, "net.fabricmc:fabric-loader:9.9.9"),
            None
        );
        assert_eq!(maven_coordinate_to_path(&libs, "not-a-coordinate"), None);

        let _ = fs::remove_dir_all(&tmp);
    }
}

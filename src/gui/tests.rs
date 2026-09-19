//! GUI unit tests (version merging, filters, sorting, name helpers).

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::updater::{self, Manifest};
use crate::version::Version;

use super::state::{
    base_mc_of, cmp_version_parts_asc, loader_of, merge_versions, sort_version_rows, VersionFilter,
    VersionRow, VersionSort,
};
use super::toast_ui::collapse_detail;

#[cfg(test)]
mod gui_tests {
    use super::*;

    #[test]
    fn collapse_detail_caps_lines_with_ellipsis() {
        // Within the cap: unchanged.
        let short = "line1\nline2\nline3";
        assert_eq!(collapse_detail(short, 3), vec!["line1", "line2", "line3"]);
        // Past the cap: the overflow becomes a single "…" line.
        let long = "a\nb\nc\nd\ne";
        assert_eq!(collapse_detail(long, 3), vec!["a", "b", "…"]);
        // A single very long line stays one line (truncation is visual).
        let one_long = "x".repeat(500);
        assert_eq!(collapse_detail(&one_long, 3), vec![one_long]);
    }

    fn local_version(name: &str) -> Version {
        Version {
            name: name.to_string(),
            dir: PathBuf::from("."),
            jar: PathBuf::from(format!("{name}.jar")),
            json: PathBuf::from(format!("{name}.json")),
        }
    }

    fn manifest_version(id: &str, kind: &str) -> updater::ManifestVersion {
        updater::ManifestVersion {
            id: id.to_string(),
            kind: kind.to_string(),
            url: format!("https://example.com/{id}.json"),
            releaseTime: String::new(),
        }
    }

    fn manifest(versions: Vec<updater::ManifestVersion>) -> Manifest {
        let mut latest = BTreeMap::new();
        latest.insert(
            "release".to_string(),
            versions
                .iter()
                .find(|v| v.kind == "release")
                .map(|v| v.id.clone())
                .unwrap_or_default(),
        );
        Manifest { latest, versions }
    }

    #[test]
    fn merge_local_first_then_remote_only() {
        let local = vec![local_version("fabric-1.20.1"), local_version("1.21.4")];
        let remote = manifest(vec![
            manifest_version("1.21.4", "release"),
            manifest_version("1.21", "release"),
            manifest_version("25w14craftmine", "snapshot"),
        ]);
        let rows = merge_versions(&local, Some(&remote));

        // Local versions come first, remote-only afterwards.
        assert_eq!(rows[0].name, "fabric-1.20.1");
        assert_eq!(rows[0].kind, "loader");
        assert_eq!(rows[0].loader, Some(updater::Loader::Fabric));
        assert!(rows[0].installed);
        assert!(rows[0].remote.is_none());

        // 1.21.4 exists in both: kind comes from the manifest, installable.
        assert_eq!(rows[1].name, "1.21.4");
        assert_eq!(rows[1].kind, "release");
        assert!(rows[1].installed);
        assert!(rows[1].remote.is_some());
        assert!(rows[1].is_latest_release);

        // Remote-only versions are not installed.
        let one_twenty_one = rows.iter().find(|r| r.name == "1.21").unwrap();
        assert!(!one_twenty_one.installed);
        assert!(one_twenty_one.remote.is_some());
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn merge_without_manifest_shows_local_only() {
        let local = vec![local_version("fabric-1.20.1")];
        let rows = merge_versions(&local, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].installed);
        assert_eq!(rows[0].kind, "loader");
    }

    #[test]
    fn merge_with_empty_local_shows_remote_only() {
        let remote = manifest(vec![manifest_version("1.21.4", "release")]);
        let rows = merge_versions(&[], Some(&remote));
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].installed);
        assert!(rows[0].remote.is_some());
    }

    #[test]
    fn filters_match_kinds_and_installed() {
        let rows = [
            VersionRow {
                name: "fabric-1.20.1".into(),
                kind: "local".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
                loader: Some(updater::Loader::Fabric),
            },
            VersionRow {
                name: "1.21.4".into(),
                kind: "release".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "25w14craftmine".into(),
                kind: "snapshot".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "a1.2.5".into(),
                kind: "old_alpha".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
        ];
        let rows = &rows[..];
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::All.matches(r))
                .count(),
            4
        );
        // The Mojang/Loaders split follows the detected loader.
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Mojang.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["1.21.4", "25w14craftmine", "a1.2.5"]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Loaders.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fabric-1.20.1"]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Release.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["1.21.4"]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Snapshot.matches(r))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Old.matches(r))
                .count(),
            1
        );
        // "Installed" includes local-only and manifest-installed releases.
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Installed.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fabric-1.20.1", "1.21.4"]
        );
    }

    #[test]
    fn loader_detection_covers_all_prefixes() {
        assert_eq!(
            loader_of("fabric-loader-0.19.5-1.21.4"),
            Some(updater::Loader::Fabric)
        );
        assert_eq!(
            loader_of("quilt-loader-0.21.0-1.20.1"),
            Some(updater::Loader::Quilt)
        );
        assert_eq!(
            loader_of("neoforge-1.21.4-21.4.157"),
            Some(updater::Loader::NeoForge)
        );
        assert_eq!(
            loader_of("forge-1.20.1-47.4.10"),
            Some(updater::Loader::Forge)
        );
        // Case-insensitive + loose fabric/forge names from other launchers.
        assert_eq!(loader_of("Fabric-1.20.1"), Some(updater::Loader::Fabric));
        assert_eq!(loader_of("1.21.4"), None);
        assert_eq!(loader_of("25w14craftmine"), None);
    }

    #[test]
    fn base_mc_extraction_strips_loader_prefixes() {
        assert_eq!(base_mc_of("fabric-loader-0.19.5-1.21.4"), "1.21.4");
        assert_eq!(base_mc_of("quilt-loader-0.21.0-1.20.1"), "1.20.1");
        assert_eq!(base_mc_of("neoforge-1.21.4-21.4.157"), "1.21.4");
        assert_eq!(base_mc_of("forge-1.20.1-47.4.10"), "1.20.1");
        assert_eq!(base_mc_of("1.20.1"), "1.20.1");
        assert_eq!(base_mc_of("25w14craftmine"), "25w14craftmine");
    }

    #[test]
    fn merge_groups_loaders_after_mojang_rows() {
        let local = vec![
            local_version("forge-1.20.1-47.4.10"),
            local_version("1.20.1"),
            local_version("fabric-loader-0.19.5-1.21.4"),
            local_version("1.21.4"),
        ];
        let rows = merge_versions(&local, None);
        // Local order is preserved; every loader row carries its loader tag.
        assert_eq!(rows[0].loader, Some(updater::Loader::Forge));
        assert_eq!(rows[1].loader, None);
        assert_eq!(rows[2].loader, Some(updater::Loader::Fabric));
        assert_eq!(rows[3].loader, None);
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Loaders.matches(r))
                .count(),
            2
        );
    }

    #[test]
    fn version_sort_orders() {
        let make = |name: &str, date: &str| VersionRow {
            name: name.into(),
            kind: "release".into(),
            installed: false,
            is_latest_release: false,
            remote: Some(updater::ManifestVersion {
                id: name.into(),
                kind: "release".into(),
                url: String::new(),
                releaseTime: date.into(),
            }),
            loader: loader_of(name),
        };
        let mut rows = vec![
            make("1.20", "2023-06-07"),
            make("fabric-loader-0.15.0-1.21", ""),
            make("1.21.4", "2024-12-03"),
            make("1.21.10", "2025-06-01"),
        ];

        // Alphabetical.
        sort_version_rows(&mut rows, VersionSort::Alphabetical);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names[0], "1.20");

        // A→Z starts with the digit-prefixed names in numeric order.
        sort_version_rows(&mut rows, VersionSort::AlphabeticalReverse);
        assert_eq!(rows.last().unwrap().name, "1.20");

        // Numeric-aware by number: 1.21.10 > 1.21.4 > 1.21... > 1.20.
        sort_version_rows(&mut rows, VersionSort::Number);
        assert_eq!(rows[0].name, "fabric-loader-0.15.0-1.21");
        assert_eq!(rows[1].name, "1.21.10");
        assert_eq!(rows[2].name, "1.21.4");
        assert_eq!(rows[3].name, "1.20");

        // By release date (newest first, rows without dates go last).
        sort_version_rows(&mut rows, VersionSort::ReleaseDate);
        assert_eq!(rows[0].name, "1.21.10");
        assert_eq!(rows[1].name, "1.21.4");
        assert_eq!(rows[2].name, "1.20");

        // By loader type: Mojang rows first, then loaders alphabetically
        // (fabric < forge).
        sort_version_rows(&mut rows, VersionSort::LoaderType);
        assert_eq!(rows[0].loader, None);
        assert_eq!(rows[1].loader, None);
        assert_eq!(rows[2].loader, None);
        assert_eq!(rows[3].loader, Some(updater::Loader::Fabric));
    }

    #[test]
    fn version_number_compare_is_numeric() {
        use std::cmp::Ordering;
        assert_eq!(cmp_version_parts_asc("1.21.9", "1.21.10"), Ordering::Less);
        assert_eq!(cmp_version_parts_asc("1.20", "1.20.1"), Ordering::Less);
        assert_eq!(cmp_version_parts_asc("1.21.4", "1.21.4"), Ordering::Equal);
        assert_eq!(cmp_version_parts_asc("a1.2", "1.2"), Ordering::Greater);
    }

    #[test]
    fn version_filter_by_loader_matches_exactly() {
        let mut rows = vec![
            VersionRow {
                name: "1.21.4".into(),
                kind: "release".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "fabric-loader-0.19.5-1.21.4".into(),
                kind: "loader".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
                loader: Some(updater::Loader::Fabric),
            },
        ];
        rows.retain(|r| r.loader == Some(updater::Loader::Fabric));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "fabric-loader-0.19.5-1.21.4");
    }
}

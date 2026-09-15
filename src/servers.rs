//! The launcher's server list, persisted as `servers.json`, with import from
//! and export to Minecraft's `servers.dat`.
//!
//! Port of the Java `ServerManager` + `ServersDatManager`. Unlike the Java
//! version, export no longer destroys unknown NBT data: existing entries keep
//! their icons and any other fields; only `name`/`ip` of managed entries are
//! updated.

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::home;
use crate::nbt::{self, Value};

pub const DEFAULT_PORT: &str = "25565";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    #[serde(default)]
    pub name: String,
    pub ip: String,
    #[serde(default = "default_port")]
    pub port: String,
}

fn default_port() -> String {
    DEFAULT_PORT.to_string()
}

impl ServerEntry {
    pub fn new(name: &str, address: &str) -> ServerEntry {
        let (ip, port) = split_address(address);
        ServerEntry {
            name: name.to_string(),
            ip,
            port,
        }
    }

    /// `ip` or `ip:port` (port omitted when it is the default).
    pub fn display_ip(&self) -> String {
        if self.port == DEFAULT_PORT || self.port.is_empty() {
            self.ip.clone()
        } else {
            format!("{}:{}", self.ip, self.port)
        }
    }
}

/// Split `host[:port]`; bare hosts get the default port. IPv6 literals in
/// `[..]:port` form are handled; bare `a:b` is treated as host:port only when
/// the tail is fully numeric.
pub fn split_address(address: &str) -> (String, String) {
    let address = address.trim();
    if let Some(rest) = address.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = rest[..end].to_string();
            let tail = &rest[end + 1..];
            let port = tail
                .strip_prefix(':')
                .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(DEFAULT_PORT);
            return (host, port.to_string());
        }
    }
    match address.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_string(), port.to_string())
        }
        _ => (address.to_string(), DEFAULT_PORT.to_string()),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStore {
    pub servers: Vec<ServerEntry>,
}

impl ServerStore {
    pub fn load(path: &Path) -> ServerStore {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => ServerStore::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn load_or_import(home_dir: &Path, game_dir: &Path) -> ServerStore {
        let path = home::servers_file(home_dir);
        let store = Self::load(&path);
        if store.servers.is_empty() {
            // First run: import the Minecraft server list, if there is one.
            let imported = read_servers_dat(&servers_dat_path(game_dir));
            if !imported.is_empty() {
                let store = ServerStore { servers: imported };
                let _ = store.save(&path);
                return store;
            }
        }
        store
    }

    pub fn add(&mut self, name: &str, address: &str) {
        self.servers.push(ServerEntry::new(name, address));
    }

    #[allow(dead_code)] // available for future UI edit-in-place
    pub fn update(&mut self, index: usize, name: &str, address: &str) {
        if let Some(entry) = self.servers.get_mut(index) {
            let (ip, port) = split_address(address);
            entry.name = name.to_string();
            entry.ip = ip;
            entry.port = port;
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.servers.len() {
            self.servers.remove(index);
        }
    }
}

/// Where Minecraft keeps `servers.dat` for a game directory.
pub fn servers_dat_path(game_dir: &Path) -> PathBuf {
    game_dir.join("servers.dat")
}

/// Read the server entries from `servers.dat` (missing file → empty list).
pub fn read_servers_dat(path: &Path) -> Vec<ServerEntry> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok((_, root)) = nbt::parse(&bytes) else {
        eprintln!(
            "[RustLauncher] failed to parse {}: keeping existing list",
            path.display()
        );
        return Vec::new();
    };
    let Some(Value::List(items)) = root.compound_get("servers") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(ip) = item.compound_get("ip").and_then(Value::as_string) else {
            continue;
        };
        if ip.is_empty() {
            continue;
        }
        let name = item
            .compound_get("name")
            .and_then(Value::as_string)
            .unwrap_or("");
        out.push(ServerEntry::new(name, ip));
    }
    out
}

/// Write the server list back to `servers.dat`.
///
/// If the file already exists, its NBT tree is loaded and only the
/// `servers` list entries' `name`/`ip` are replaced — icons and any other
/// fields survive. Missing files are created fresh.
pub fn write_servers_dat(path: &Path, servers: &[ServerEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root = match std::fs::read(path) {
        Ok(bytes) => match nbt::parse(&bytes) {
            Ok((_, root @ Value::Compound(_))) => root,
            _ => Value::Compound(vec![]),
        },
        Err(_) => Value::Compound(vec![]),
    };

    let list = build_servers_list(path, servers);
    set_in_compound(&mut root, "servers", list);

    let bytes = nbt::write("", &root);
    // Write via a temp file so a crash cannot truncate a valid servers.dat.
    let tmp = path.with_extension("dat.tmp");
    {
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(&bytes)?;
        file.sync_all().ok();
    }
    std::fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// Build the `servers` list, reusing an existing NBT entry (with its icon)
/// for every server whose address is unchanged, so untouched servers keep
/// all their extra fields.
fn build_servers_list(path: &Path, servers: &[ServerEntry]) -> Value {
    let existing: Vec<Value> = match std::fs::read(path) {
        Ok(bytes) => match nbt::parse(&bytes) {
            Ok((_, root)) => match root.compound_get("servers") {
                Some(Value::List(items)) => items.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        },
        Err(_) => Vec::new(),
    };

    let mut items = Vec::with_capacity(servers.len());
    for server in servers {
        // Find a previous entry with the same address to preserve its icon.
        let reused = existing.iter().find(|entry| {
            entry
                .compound_get("ip")
                .and_then(Value::as_string)
                .map(|ip| same_address(ip, server))
                .unwrap_or(false)
        });
        let mut fields = match reused {
            Some(Value::Compound(fields)) => fields
                .iter()
                .filter(|(k, _)| k != "name" && k != "ip")
                .cloned()
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        // name/ip go first, matching the vanilla file layout.
        fields.insert(0, ("ip".to_string(), Value::String(server.display_ip())));
        fields.insert(0, ("name".to_string(), Value::String(server.name.clone())));
        items.push(Value::Compound(fields));
    }
    Value::List(items)
}

/// Compare a stored `ip` value with a launcher entry regardless of whether
/// the default port is spelled out.
fn same_address(stored: &str, server: &ServerEntry) -> bool {
    let (ip, port) = split_address(stored);
    ip == server.ip && (port == server.port || (port == DEFAULT_PORT && server.port.is_empty()))
}

/// Set (or append) a key in a compound, preserving field order.
fn set_in_compound(root: &mut Value, key: &str, value: Value) {
    if let Value::Compound(entries) = root {
        if let Some(slot) = entries.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            entries.push((key.to_string(), value));
        }
    }
}

/// TCP reachability probe for the Servers screen.
pub fn check_status(address: &str) -> ServerStatus {
    let (ip, port) = split_address(address);
    let Ok(port_num) = port.parse::<u16>() else {
        return ServerStatus::Offline("invalid port".into());
    };
    let addr = format!("{ip}:{port_num}");
    let timeout = Duration::from_secs(3);
    match TcpStream::connect_timeout(
        &addr
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap()),
        timeout,
    ) {
        Ok(_) => ServerStatus::Online,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            ServerStatus::Offline("connection refused".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            ServerStatus::Offline("timeout".into())
        }
        Err(e) => ServerStatus::Offline(e.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    Online,
    Offline(String),
}

impl std::fmt::Display for ServerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerStatus::Online => write!(f, "online"),
            ServerStatus::Offline(reason) => write!(f, "offline ({reason})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-srv-{}-{tag}", std::process::id()))
    }

    #[test]
    fn splits_addresses() {
        assert_eq!(
            split_address("mc.example.com"),
            ("mc.example.com".into(), "25565".into())
        );
        assert_eq!(
            split_address("mc.example.com:25566"),
            ("mc.example.com".into(), "25566".into())
        );
        assert_eq!(split_address("[::1]:25565"), ("::1".into(), "25565".into()));
        assert_eq!(
            split_address("[2001:db8::1]"),
            ("2001:db8::1".into(), "25565".into())
        );
        assert_eq!(
            split_address("host:notaport"),
            ("host:notaport".into(), "25565".into())
        );
    }

    #[test]
    fn display_ip_hides_default_port() {
        let plain = ServerEntry::new("A", "mc.example.com");
        assert_eq!(plain.display_ip(), "mc.example.com");
        let custom = ServerEntry::new("B", "127.0.0.1:25566");
        assert_eq!(custom.display_ip(), "127.0.0.1:25566");
    }

    #[test]
    fn add_update_remove() {
        let mut store = ServerStore::default();
        store.add("A", "a.example.com:25566");
        store.add("B", "b.example.com");
        assert_eq!(store.servers.len(), 2);
        store.update(0, "A2", "c.example.com");
        assert_eq!(store.servers[0].name, "A2");
        assert_eq!(store.servers[0].ip, "c.example.com");
        assert_eq!(store.servers[0].port, DEFAULT_PORT);
        store.remove(1);
        assert_eq!(store.servers.len(), 1);
    }

    #[test]
    fn json_roundtrip() {
        let path = tmp("json").join("servers.json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let mut store = ServerStore::default();
        store.add("Hypixel", "mc.hypixel.net");
        store.save(&path).unwrap();
        let loaded = ServerStore::load(&path);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn dat_write_then_read_and_icons_survive() {
        let dir = tmp("icons");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("servers.dat");

        // First write: two servers.
        write_servers_dat(
            &path,
            &[
                ServerEntry::new("Hypixel", "mc.hypixel.net"),
                ServerEntry::new("Local", "127.0.0.1:25566"),
            ],
        )
        .unwrap();
        let read = read_servers_dat(&path);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].name, "Hypixel");
        assert_eq!(read[1].port, "25566");

        // Inject an icon into the first entry, like a real client would have.
        let bytes = std::fs::read(&path).unwrap();
        let (_, mut root) = nbt::parse(&bytes).unwrap();
        if let Value::Compound(entries) = &mut root {
            for (k, v) in entries.iter_mut() {
                if k == "servers" {
                    if let Value::List(list) = v {
                        if let Value::Compound(fields) = &mut list[0] {
                            fields.push((
                                "icon".to_string(),
                                Value::ByteArray(vec![0x89, b'P', b'N', b'G']),
                            ));
                        }
                    }
                }
            }
        }
        std::fs::write(&path, nbt::write("", &root)).unwrap();

        // Second write: edit the list but keep the same addresses.
        write_servers_dat(
            &path,
            &[
                ServerEntry::new("Hypixel Renamed", "mc.hypixel.net"),
                ServerEntry::new("Local", "127.0.0.1:25566"),
                ServerEntry::new("Added", "new.example.com"),
            ],
        )
        .unwrap();

        // The icon must survive the edit (this is what the Java version lost).
        let bytes = std::fs::read(&path).unwrap();
        let (_, root) = nbt::parse(&bytes).unwrap();
        if let Value::List(items) = root.compound_get("servers").unwrap() {
            let has_icon = items[0]
                .compound_get("icon")
                .map(|v| matches!(v, Value::ByteArray(b) if b == &vec![0x89, b'P', b'N', b'G']))
                .unwrap_or(false);
            assert!(has_icon, "icon must survive list edit");
            assert_eq!(
                items[0].compound_get("name").unwrap().as_string().unwrap(),
                "Hypixel Renamed"
            );
        } else {
            panic!("servers must be a list");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_dat_is_empty_not_error() {
        assert!(read_servers_dat(Path::new("Z:/no/such/servers.dat")).is_empty());
    }

    #[test]
    fn fresh_dat_file_is_valid() {
        let dir = tmp("fresh");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("servers.dat");
        write_servers_dat(&path, &[ServerEntry::new("Only", "1.2.3.4")]).unwrap();
        let read = read_servers_dat(&path);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].ip, "1.2.3.4");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

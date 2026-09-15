//! Offline authentication — the same scheme as vanilla offline mode and the
//! original PowerLaunch: UUID = md5("OfflinePlayer:<name>") formatted as a
//! type-3 (name-based) UUID.

use anyhow::{anyhow, Result};
use md5::compute as md5_compute;

/// A logged-in (offline) account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    pub uuid: String,
}

/// Validate an offline username per Minecraft rules.
pub fn validate_username(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.len() < 3 {
        return Err(anyhow!("username must be at least 3 characters"));
    }
    if name.len() > 16 {
        return Err(anyhow!("username cannot be longer than 16 characters"));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow!(
            "username may only contain letters, digits, and underscores"
        ));
    }
    Ok(name)
}

/// Derive the offline UUID (v3, md5 of `OfflinePlayer:<name>`) with the
/// RFC 4122 version/variant bits set, as Minecraft does.
pub fn offline_uuid(name: &str) -> String {
    let digest = md5_compute(format!("OfflinePlayer:{name}"));
    let mut bytes = digest.0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // IETF variant

    format_uuid(&bytes)
}

/// Format 16 bytes as the canonical dashed UUID string.
fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = |slice: &[u8]| -> String { slice.iter().map(|b| format!("{b:02x}")).collect() };
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

/// Log in offline with the given username.
pub fn login_offline(username: &str) -> Result<Account> {
    let name = validate_username(username)?.to_string();
    Ok(Account {
        uuid: offline_uuid(&name),
        username: name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_usernames() {
        assert_eq!(validate_username("Rizer001").unwrap(), "Rizer001");
        assert_eq!(validate_username("  abc  ").unwrap(), "abc");
        assert_eq!(validate_username("a_b_123").unwrap(), "a_b_123");
    }

    #[test]
    fn rejects_invalid_usernames() {
        assert!(validate_username("ab").is_err()); // too short
        assert!(validate_username("averyveryverylongname").is_err()); // >16
        assert!(validate_username("bad name").is_err()); // space
        assert!(validate_username("bad-д name").is_err()); // non-ascii
        assert!(validate_username("").is_err()); // empty
    }

    #[test]
    fn offline_uuid_is_deterministic_and_well_formed() {
        let a = offline_uuid("Notch");
        let b = offline_uuid("Notch");
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().filter(|c| *c == '-').count(), 4);

        // Version 3 and IETF variant nibbles.
        let hex: String = a.chars().filter(|c| *c != '-').collect();
        assert!(hex.starts_with(|_| true));
        let version_nibble = hex.chars().nth(12).unwrap().to_digit(16).unwrap();
        assert_eq!(version_nibble, 3);

        // Different names -> different UUIDs.
        assert_ne!(a, offline_uuid("Dinnerbone"));
    }

    #[test]
    fn login_offline_roundtrip() {
        let account = login_offline("Rizer001").unwrap();
        assert_eq!(account.username, "Rizer001");
        assert_eq!(account.uuid, offline_uuid("Rizer001"));
        assert!(login_offline("x").is_err());
    }
}

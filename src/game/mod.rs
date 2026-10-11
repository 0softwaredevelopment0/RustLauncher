//! The game launch pipeline: version discovery, `version.json` parsing,
//! classpath assembly, JVM flags, instances and Java runtime management.

pub mod classpath;
pub mod instances;
pub mod java;
pub mod jvm;
pub mod launcher;
pub mod version;
pub mod version_json;

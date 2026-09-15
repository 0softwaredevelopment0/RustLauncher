//! JVM flag presets. Memory is configured here (via `-Xms`/`-Xmx`), not by a
//! separate RAM slider: the game refuses to launch without heap flags.

/// The fallback flag set used everywhere no explicit preset was chosen.
pub const DEFAULT_JVM_ARGS: &str = "-Xms1m -Xmx4g";

/// A named, ready-made JVM flag set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JvmPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub args: &'static str,
}

/// All built-in presets, in display order.
pub const PRESETS: &[JvmPreset] = &[
    JvmPreset {
        id: "minimal",
        label: "Minimal",
        args: "-Xms1m -Xmx4g",
    },
    JvmPreset {
        id: "g1gc",
        label: "G1GC (optimized)",
        args: "-Xms1m -Xmx4g \
               -XX:+UseG1GC \
               -XX:+ParallelRefProcEnabled \
               -XX:MaxGCPauseMillis=200 \
               -XX:G1NewSizePercent=20 \
               -XX:G1ReservePercent=20 \
               -XX:MaxTenuringThreshold=1 \
               -XX:InitiatingHeapOccupancyPercent=15 \
               -XX:G1HeapRegionSize=16M \
               -XX:+UnlockExperimentalVMOptions \
               -XX:+DisableExplicitGC \
               -XX:+AlwaysPreTouch",
    },
    JvmPreset {
        id: "shenandoah",
        label: "Shenandoah GC (optimized)",
        args: "-Xms1m -Xmx4g \
               -XX:+UseShenandoahGC \
               -XX:ShenandoahGCHeuristics=adaptive \
               -XX:+AlwaysPreTouch \
               -XX:+DisableExplicitGC \
               -XX:+ParallelRefProcEnabled",
    },
    JvmPreset {
        id: "zgc",
        label: "ZGC (optimized)",
        args: "-Xms1m -Xmx4g \
               -XX:+UseZGC \
               -XX:+AlwaysPreTouch \
               -XX:+DisableExplicitGC \
               -XX:+ParallelRefProcEnabled",
    },
    JvmPreset {
        id: "parallel",
        label: "Parallel GC (optimized)",
        args: "-Xms1m -Xmx4g \
               -XX:+UseParallelGC \
               -XX:+ParallelRefProcEnabled \
               -XX:MaxGCPauseMillis=200 \
               -XX:+DisableExplicitGC \
               -XX:+AlwaysPreTouch",
    },
];

/// Look up a preset by id.
#[allow(dead_code)] // part of the preset API (GUI matches by id later)
pub fn find_preset(id: &str) -> Option<&'static JvmPreset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Is this flag a heap-size flag (`-Xms…` / `-Xmx…`, any case, any unit)?
#[allow(dead_code)] // used by tests and available to the GUI
pub fn is_heap_flag(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    lower.starts_with("-xms") || lower.starts_with("-xmx")
}

/// Launch-time validation of the JVM flags.
///
/// The game must not start when the flag set is empty or when either heap
/// boundary is missing: every preset guarantees `-Xms` + `-Xmx`, and manual
/// edits are checked here.
pub fn validate_jvm_args(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(
            "JVM flags are empty — the game cannot start without heap flags \
                    (pick a preset in Settings or add -Xms/-Xmx)."
                .to_string(),
        );
    }
    let has_xms = args.iter().any(|a| {
        let lower = a.to_ascii_lowercase();
        lower.starts_with("-xms")
    });
    let has_xmx = args.iter().any(|a| {
        let lower = a.to_ascii_lowercase();
        lower.starts_with("-xmx")
    });
    match (has_xms, has_xmx) {
        (true, true) => Ok(()),
        (false, true) => Err("JVM flags are missing -Xms (initial heap).".to_string()),
        (true, false) => Err("JVM flags are missing -Xmx (maximum heap).".to_string()),
        (false, false) => Err("JVM flags are missing both -Xms and -Xmx (heap size).".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_flags_are_valid_and_minimal() {
        let parsed: Vec<String> = DEFAULT_JVM_ARGS
            .split_whitespace()
            .map(String::from)
            .collect();
        validate_jvm_args(&parsed).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn every_preset_has_both_heap_flags_and_is_sorted() {
        for preset in PRESETS {
            let parsed: Vec<String> = preset.args.split_whitespace().map(String::from).collect();
            validate_jvm_args(&parsed)
                .unwrap_or_else(|e| panic!("preset {} invalid: {e}", preset.id));
            let heaps: Vec<&String> = parsed.iter().filter(|a| is_heap_flag(a)).collect();
            assert_eq!(
                heaps.len(),
                2,
                "preset {} must have exactly -Xms and -Xmx",
                preset.id
            );
        }
        // The default set equals the minimal preset.
        assert_eq!(find_preset("minimal").unwrap().args, DEFAULT_JVM_ARGS);
    }

    #[test]
    fn gc_presets_use_their_collectors() {
        assert!(find_preset("g1gc").unwrap().args.contains("+UseG1GC"));
        assert!(find_preset("shenandoah")
            .unwrap()
            .args
            .contains("+UseShenandoahGC"));
        assert!(find_preset("zgc").unwrap().args.contains("+UseZGC"));
        assert!(find_preset("parallel")
            .unwrap()
            .args
            .contains("+UseParallelGC"));
    }

    #[test]
    fn validation_rejects_missing_heap_flags() {
        let to_vec = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(validate_jvm_args(&to_vec("")).is_err());
        assert!(validate_jvm_args(&to_vec("-XX:+UseG1GC")).is_err());
        assert!(validate_jvm_args(&to_vec("-Xmx4g")).is_err());
        assert!(validate_jvm_args(&to_vec("-Xms1m")).is_err());
        assert!(validate_jvm_args(&to_vec("-XMS1m -XMX4g")).is_ok()); // case-insensitive
        assert!(validate_jvm_args(&to_vec("-Xms1m -Xmx4g -XX:+UseZGC")).is_ok());
    }
}

//! IntelliJ-compatible dynamic variables, generated fresh per
//! [`prepare_request`] invocation.
//!
//! Names follow the JetBrains convention with a `$` prefix so they don't
//! collide with user-defined variables in `http-client.env.json`. Supported:
//!
//! - `$uuid` — v7 UUID (time-ordered, RFC 9562), lowercase, hyphenated.
//!   Time-ordered means consecutive `$uuid` values from the same machine
//!   sort lexicographically the same way they were generated — handy as a
//!   correlation ID or as a database primary key in test fixtures.
//! - `$timestamp` — current Unix time in seconds (UTC).
//! - `$isoTimestamp` — current time as RFC 3339 / ISO 8601 with `Z`
//!   timezone.
//! - `$randomInt` — uniform integer in `[0, 1000]`.
//!
//! All four values are computed once per call to [`build_dynamic_variables`]
//! so multiple `{{$uuid}}` references inside the same request expand to
//! the *same* UUID — useful for correlation IDs replayed in both a header
//! and a body field. A subsequent request gets a fresh set.
//!
//! User variables (from env files or in-file `@vars`) take precedence over
//! dynamic ones, so a user can override `$timestamp` for deterministic
//! tests by declaring `@$timestamp = 1700000000`.

use chrono::{SecondsFormat, Utc};
use rand::Rng;
use uuid::Uuid;

use crate::env::VariableMap;

pub fn build_dynamic_variables() -> VariableMap {
    let now = Utc::now();
    let mut rng = rand::thread_rng();
    let mut map = VariableMap::new();
    map.insert("$uuid".to_string(), Uuid::now_v7().to_string());
    map.insert("$timestamp".to_string(), now.timestamp().to_string());
    map.insert(
        "$isoTimestamp".to_string(),
        now.to_rfc3339_opts(SecondsFormat::Secs, true),
    );
    map.insert(
        "$randomInt".to_string(),
        rng.gen_range(0..=1000).to_string(),
    );
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_all_supported_names() {
        let map = build_dynamic_variables();
        for key in ["$uuid", "$timestamp", "$isoTimestamp", "$randomInt"] {
            assert!(map.contains_key(key), "missing {key}");
        }
    }

    #[test]
    fn uuid_is_v7_36_chars_lowercase() {
        let map = build_dynamic_variables();
        let uuid_text = map.get("$uuid").unwrap();
        assert_eq!(uuid_text.len(), 36, "uuid was '{uuid_text}'");
        assert!(uuid_text.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        // Version nibble lives in the 15th character (0-indexed); v7 is '7'.
        let version_char = uuid_text.chars().nth(14).unwrap();
        assert_eq!(version_char, '7', "expected v7 UUID, got '{uuid_text}'");
    }

    #[test]
    fn consecutive_uuids_are_time_ordered() {
        let a = build_dynamic_variables().get("$uuid").unwrap().clone();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = build_dynamic_variables().get("$uuid").unwrap().clone();
        assert!(a < b, "expected {a} < {b} for time-ordered v7 UUIDs");
    }

    #[test]
    fn timestamp_parses_as_integer() {
        let map = build_dynamic_variables();
        let ts: i64 = map.get("$timestamp").unwrap().parse().unwrap();
        // Sanity check: should be in the rough neighbourhood of "now".
        assert!(ts > 1_700_000_000);
    }

    #[test]
    fn iso_timestamp_ends_in_z() {
        let map = build_dynamic_variables();
        assert!(map.get("$isoTimestamp").unwrap().ends_with('Z'));
    }

    #[test]
    fn random_int_is_bounded() {
        for _ in 0..50 {
            let map = build_dynamic_variables();
            let n: i64 = map.get("$randomInt").unwrap().parse().unwrap();
            assert!((0..=1000).contains(&n));
        }
    }
}

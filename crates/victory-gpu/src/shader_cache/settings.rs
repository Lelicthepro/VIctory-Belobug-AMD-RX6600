//! Cache-key encoding for optional translator switches.
use std::ffi::OsStr;

pub(super) fn append(out: &mut Vec<u8>, name: &str, value: Option<&OsStr>) {
    out.extend_from_slice(name.as_bytes());
    out.push(b'=');
    if let Some(value) = value {
        let bytes = value.as_encoded_bytes();
        // Keep existing keys for absent and nonempty settings. Presence-only
        // switches (STRICT_JOINS, MESH_OUTPUT_STRICT) also accept an empty
        // value, which must not alias absence. NUL cannot occur in an OS
        // environment value, so this marker cannot alias a nonempty value.
        if bytes.is_empty() {
            out.push(0);
        }
        out.extend_from_slice(bytes);
    }
    out.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(value: Option<&str>) -> Vec<u8> {
        let mut out = Vec::new();
        append(&mut out, "VICTORY_STRICT_JOINS", value.map(OsStr::new));
        out
    }
    #[test]
    fn presence_only_switch_has_distinct_keys() {
        assert_ne!(key(None), key(Some("")));
        assert_ne!(key(Some("")), key(Some("0")));
        assert_ne!(key(Some("0")), key(Some("1")));
    }
    #[test]
    fn existing_absent_and_nonempty_keys_are_preserved() {
        assert_eq!(key(None), b"VICTORY_STRICT_JOINS=\0");
        assert_eq!(key(Some("1")), b"VICTORY_STRICT_JOINS=1\0");
    }
}

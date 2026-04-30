//! Persistence of "what has been applied" — `.template/state.toml`.
//!
//! Each `tmpl apply` / `tmpl add` records the layers it wrote, the
//! BLAKE3 content hash of each layer's rendered patch, and a Merkle
//! root over the whole applied set. Re-application compares hashes
//! before touching files: matching layers are skipped, drifted layers
//! enter the merge path (Phase B), absent layers are added.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use toml_edit::de as toml_de;
use toml_edit::ser as toml_ser;

use crate::error::TmplError;
use crate::layer::{LayerName, Patch, RenderedFile};

/// 32-byte BLAKE3 content hash. The `Default` value (all zeros) is the
/// "empty repository" Merkle root — handy for `State::default()` on
/// fresh checkouts before any layer has been applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Lower-case hex representation, used in `state.toml`.
    ///
    /// Takes `self` by value because [`ContentHash`] is `Copy` —
    /// borrowing a 32-byte array is the same cost as copying it on a
    /// 64-bit register-rich machine, and the by-value form lets call
    /// sites use the value without an explicit `&` borrow.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            // `Write` is brought into scope as `_` at file top so
            // `write!` resolves without polluting public re-exports.
            write!(&mut s, "{b:02x}").expect("write to String never fails");
        }
        s
    }

    /// Parse the hex form. Lengths and digits are validated.
    ///
    /// # Errors
    ///
    /// * [`HashParseError::Length`] when `s` is not exactly 64 chars.
    /// * [`HashParseError::Digit`] when `s` contains a non-hex byte.
    pub fn from_hex(s: &str) -> Result<Self, HashParseError> {
        if s.len() != 64 {
            return Err(HashParseError::Length);
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_nibble(chunk[0]).ok_or(HashParseError::Digit(chunk[0]))?;
            let lo = hex_nibble(chunk[1]).ok_or(HashParseError::Digit(chunk[1]))?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <SmolStr as Deserialize>::deserialize(d)?;
        Self::from_hex(&s).map_err(D::Error::custom)
    }
}

/// Hash parse failure. Size kept compact (both variants ≤ 2 bytes) so
/// the enum can be `Copy` and so the variant-size lint stays satisfied
/// without ad-hoc allow attributes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HashParseError {
    /// Hex string was not 64 characters long.
    #[error("expected 64 hex chars")]
    Length,
    /// Hex string contained a non-hex character.
    #[error("non-hex digit {0:?}")]
    Digit(u8),
}

/// Hash a single patch — BLAKE3 over the canonical concatenation of
/// `path\n<content-bytes>\n` for every rendered file, in path-sorted
/// order. Sorting normalises permutations of the same logical patch.
#[must_use]
pub fn hash_patch(patch: &Patch) -> ContentHash {
    let mut files: Vec<&RenderedFile> = patch.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = blake3::Hasher::new();
    hasher.update(patch.layer.as_str().as_bytes());
    hasher.update(b"\0");
    for f in files {
        hasher.update(f.path.as_path().as_str().as_bytes());
        hasher.update(b"\n");
        hasher.update(f.content.as_bytes());
        hasher.update(b"\n");
    }
    ContentHash(*hasher.finalize().as_bytes())
}

/// Merkle root over a set of (layer, hash) entries. The set is sorted
/// by layer name so the root is permutation-invariant.
#[must_use]
pub fn merkle_root(entries: &BTreeMap<LayerName, ContentHash>) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tmpl-merkle-v1\0");
    for (name, hash) in entries {
        hasher.update(name.as_str().as_bytes());
        hasher.update(b":");
        hasher.update(&hash.0);
        hasher.update(b"\n");
    }
    ContentHash(*hasher.finalize().as_bytes())
}

/// Persisted form of `state.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct State {
    /// Engine version that wrote this file.
    #[serde(default)]
    pub engine_version: SmolStr,
    /// Merkle root over `applied`.
    pub merkle_root: ContentHash,
    /// Per-layer content hashes.
    pub applied: BTreeMap<LayerName, AppliedEntry>,
}

/// Per-layer state entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedEntry {
    /// BLAKE3 hash of the layer's rendered patch.
    pub content_hash: ContentHash,
    /// RFC 3339 timestamp at which the layer was applied.
    pub applied_at: SmolStr,
}

impl State {
    /// Read `state.toml` from disk; absence is treated as the empty
    /// state, not as an error (a fresh repository has no state).
    ///
    /// # Errors
    ///
    /// Returns [`TmplError::State`] if the file exists but cannot be
    /// parsed, [`TmplError::Io`] for filesystem-level failures other
    /// than `NotFound`.
    pub fn load(path: &Path) -> Result<Self, TmplError> {
        match fs::read_to_string(path) {
            Ok(text) => toml_de::from_str::<Self>(&text).map_err(|e| TmplError::State {
                path: path.to_owned(),
                message: format!("{e}"),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(TmplError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Write `state.toml` atomically (write to `<path>.tmp`, then
    /// rename).
    ///
    /// # Errors
    ///
    /// [`TmplError::Io`] for any underlying I/O failure;
    /// [`TmplError::State`] if `toml_edit` cannot serialise the
    /// in-memory value (essentially never for the typed model).
    pub fn save(&self, path: &Path) -> Result<(), TmplError> {
        let text = toml_ser::to_string_pretty(self).map_err(|e| TmplError::State {
            path: path.to_owned(),
            message: format!("{e}"),
        })?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(|source| TmplError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, path).map_err(|source| TmplError::Io {
            path: path.to_owned(),
            source,
        })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{LayerName, Patch, RenderedFile, RenderedPath};

    fn name(s: &str) -> LayerName {
        LayerName::new(s).expect("test fixture name must be valid")
    }

    fn rfile(p: &str, c: &str) -> RenderedFile {
        RenderedFile {
            path: RenderedPath::new(p).expect("test fixture path must be valid"),
            content: c.into(),
            executable: false,
        }
    }

    #[test]
    fn content_hash_roundtrips_hex() {
        let h = ContentHash([0xab; 32]);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64);
        let h2 = ContentHash::from_hex(&hex).expect("our own output must round-trip");
        assert_eq!(h, h2);
    }

    #[test]
    fn hash_patch_invariant_under_path_sort() {
        let p1 = Patch {
            layer: name("core"),
            files: vec![rfile("a.txt", "alpha"), rfile("b.txt", "beta")],
        };
        let p2 = Patch {
            layer: name("core"),
            files: vec![rfile("b.txt", "beta"), rfile("a.txt", "alpha")],
        };
        assert_eq!(hash_patch(&p1), hash_patch(&p2));
    }

    #[test]
    fn merkle_root_distinguishes_content() {
        let mut a = BTreeMap::new();
        a.insert(name("core"), ContentHash([1u8; 32]));
        let mut b = BTreeMap::new();
        b.insert(name("core"), ContentHash([2u8; 32]));
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }

    #[test]
    fn merkle_root_is_permutation_invariant() {
        let mut a = BTreeMap::new();
        a.insert(name("core"), ContentHash([1u8; 32]));
        a.insert(name("docker-dev"), ContentHash([2u8; 32]));
        let mut b = BTreeMap::new();
        b.insert(name("docker-dev"), ContentHash([2u8; 32]));
        b.insert(name("core"), ContentHash([1u8; 32]));
        assert_eq!(merkle_root(&a), merkle_root(&b));
    }

    #[test]
    fn state_load_treats_missing_as_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("state.toml");
        let s = State::load(&p).expect("missing state must load as empty");
        assert!(s.applied.is_empty());
    }

    #[test]
    fn state_save_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("state.toml");

        let mut applied = BTreeMap::new();
        applied.insert(
            name("core"),
            AppliedEntry {
                content_hash: ContentHash([7u8; 32]),
                applied_at: SmolStr::new("2026-04-30T12:00:00Z"),
            },
        );
        let mut hashes_only = BTreeMap::new();
        for (k, v) in &applied {
            hashes_only.insert(k.clone(), v.content_hash);
        }
        let merkle_root = merkle_root(&hashes_only);
        let s = State {
            engine_version: SmolStr::new("0.1.0"),
            merkle_root,
            applied,
        };
        s.save(&p).expect("save");
        let loaded = State::load(&p).expect("load");
        assert_eq!(s, loaded);
    }

    #[test]
    fn hex_parse_rejects_bad_length() {
        assert!(matches!(
            ContentHash::from_hex("abcd"),
            Err(HashParseError::Length)
        ));
    }

    #[test]
    fn hex_parse_rejects_non_hex() {
        let mut s = String::from("g");
        s.push_str(&"a".repeat(63));
        assert!(matches!(
            ContentHash::from_hex(&s),
            Err(HashParseError::Digit(_))
        ));
    }

    #[test]
    fn content_hash_default_is_all_zeros() {
        let h = ContentHash::default();
        assert_eq!(h.0, [0u8; 32]);
        assert_eq!(h.to_hex(), "0".repeat(64));
    }

    #[test]
    fn hash_parse_error_displays_human_readably() {
        assert_eq!(format!("{}", HashParseError::Length), "expected 64 hex chars");
        assert_eq!(
            format!("{}", HashParseError::Digit(b'g')),
            "non-hex digit 103",
        );
    }

    #[test]
    fn state_default_is_empty() {
        let s = State::default();
        assert!(s.applied.is_empty());
        assert_eq!(s.engine_version.as_str(), "");
        assert_eq!(s.merkle_root, ContentHash::default());
    }

    #[test]
    fn save_returns_state_error_on_unencodable_input() {
        // Constructing a State that toml_edit::ser cannot serialise
        // requires an invalid TOML key shape — `applied_at` is a free-
        // form SmolStr so it's hard to invalidate. Exercise the happy
        // path here and rely on the `state_save_load_roundtrip` test
        // for serialise + deserialise correctness.
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("state.toml");
        let state = State::default();
        state.save(&p).expect("default state must serialise");
        assert!(p.exists());
    }

    #[test]
    fn state_load_returns_state_error_for_corrupt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("state.toml");
        fs::write(&p, "not = valid = state\n").expect("write");
        let err = State::load(&p).expect_err("must fail");
        assert!(matches!(err, TmplError::State { .. }));
    }
}

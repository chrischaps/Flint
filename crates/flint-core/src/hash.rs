//! Content-based hashing for change detection

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

/// A SHA-256 based content hash for detecting changes.
///
/// Used to track whether scene files or other content has changed,
/// enabling efficient incremental updates.
#[derive(Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Compute a hash from bytes
    pub fn from_bytes(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        Self(result.into())
    }

    /// Compute a hash from a string
    pub fn from_str(s: &str) -> Self {
        Self::from_bytes(s.as_bytes())
    }

    /// Get the hash as a hex string
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Compute a hash from a file's contents
    pub fn from_file<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(Self::from_bytes(&data))
    }

    /// Get the hash as a prefixed hex string (e.g., "sha256:abcdef...")
    pub fn to_prefixed_hex(&self) -> String {
        format!("sha256:{}", self.to_hex())
    }

    /// Parse a prefixed hex string back into a ContentHash
    pub fn from_prefixed_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix("sha256:")?;
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_hex()[..16])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consistent_hashing() {
        let h1 = ContentHash::from_str("hello");
        let h2 = ContentHash::from_str("hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_different_content_different_hash() {
        let h1 = ContentHash::from_str("hello");
        let h2 = ContentHash::from_str("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hex_output() {
        let h = ContentHash::from_str("hello");
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64); // 32 bytes * 2 hex chars
    }

    #[test]
    fn test_prefixed_hex_roundtrip() {
        let h = ContentHash::from_str("test data");
        let prefixed = h.to_prefixed_hex();
        assert!(prefixed.starts_with("sha256:"));
        let parsed = ContentHash::from_prefixed_hex(&prefixed).unwrap();
        assert_eq!(h, parsed);
    }

    #[test]
    fn test_from_prefixed_hex_invalid() {
        assert!(ContentHash::from_prefixed_hex("md5:abc").is_none());
        assert!(ContentHash::from_prefixed_hex("sha256:tooshort").is_none());
    }
}

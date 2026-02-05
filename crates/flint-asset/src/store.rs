//! Content-addressed asset storage

use flint_core::{ContentHash, FlintError, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Content-addressed file storage
///
/// Stores files at `.flint/assets/<first-2-hex>/<full-hash>.<ext>`
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    /// Create a new content store at the given root directory
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Store a file and return its content hash
    pub fn store<P: AsRef<Path>>(&self, source_path: P) -> Result<ContentHash> {
        let source = source_path.as_ref();
        let hash = ContentHash::from_file(source)
            .map_err(|e| FlintError::AssetError(format!("Failed to hash file: {}", e)))?;

        let dest = self.path_for_hash(&hash, source);
        if dest.exists() {
            return Ok(hash); // Already stored (dedup)
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::copy(source, &dest)?;
        Ok(hash)
    }

    /// Get the storage path for a hash
    pub fn get(&self, hash: &ContentHash) -> Option<PathBuf> {
        let hex = hash.to_hex();
        let prefix = &hex[..2];
        let dir = self.root.join(prefix);

        if !dir.exists() {
            return None;
        }

        // Look for any file matching the hash
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with(&hex) {
                    return Some(entry.path());
                }
            }
        }

        None
    }

    /// Check if a hash exists in the store
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.get(hash).is_some()
    }

    /// List all stored asset hashes
    pub fn list(&self) -> Result<Vec<ContentHash>> {
        let mut hashes = Vec::new();

        if !self.root.exists() {
            return Ok(hashes);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                for file_entry in fs::read_dir(entry.path())? {
                    let file_entry = file_entry?;
                    let name = file_entry.file_name();
                    let name_str = name.to_string_lossy();
                    // Extract hash from filename (before the extension)
                    if let Some(hash_hex) = name_str.split('.').next() {
                        if let Some(hash) = ContentHash::from_prefixed_hex(&format!("sha256:{}", hash_hex)) {
                            hashes.push(hash);
                        }
                    }
                }
            }
        }

        Ok(hashes)
    }

    /// Remove a stored asset
    pub fn remove(&self, hash: &ContentHash) -> Result<bool> {
        if let Some(path) = self.get(hash) {
            fs::remove_file(path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Build the storage path for a hash with extension from source
    fn path_for_hash(&self, hash: &ContentHash, source: &Path) -> PathBuf {
        let hex = hash.to_hex();
        let prefix = &hex[..2];
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");

        self.root.join(prefix).join(format!("{}.{}", hex, ext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_store_and_retrieve() {
        let store_dir = temp_dir();
        let store = ContentStore::new(&store_dir);

        // Create a test file
        let test_file = store_dir.join("test.txt");
        let mut f = fs::File::create(&test_file).unwrap();
        f.write_all(b"hello world").unwrap();

        let hash = store.store(&test_file).unwrap();
        assert!(store.contains(&hash));

        let retrieved = store.get(&hash).unwrap();
        let content = fs::read_to_string(retrieved).unwrap();
        assert_eq!(content, "hello world");

        // Cleanup
        fs::remove_dir_all(&store_dir).ok();
    }

    #[test]
    fn test_store_dedup() {
        let store_dir = temp_dir();
        let store = ContentStore::new(&store_dir);

        let test_file = store_dir.join("test.txt");
        fs::write(&test_file, b"same content").unwrap();

        let hash1 = store.store(&test_file).unwrap();
        let hash2 = store.store(&test_file).unwrap();
        assert_eq!(hash1, hash2);

        fs::remove_dir_all(&store_dir).ok();
    }

    #[test]
    fn test_remove() {
        let store_dir = temp_dir();
        let store = ContentStore::new(&store_dir);

        let test_file = store_dir.join("test.txt");
        fs::write(&test_file, b"to be removed").unwrap();

        let hash = store.store(&test_file).unwrap();
        assert!(store.contains(&hash));

        assert!(store.remove(&hash).unwrap());
        assert!(!store.contains(&hash));

        fs::remove_dir_all(&store_dir).ok();
    }
}

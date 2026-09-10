//! Build cache — skips a pipeline stage entirely when nothing that
//! could affect its output has changed since the last successful run.
//!
//! Design: each cacheable stage computes a fingerprint (a SHA-256 hash
//! over every input that could change its output: source file
//! contents, relevant `plugin.json` fields, and the toolchain binary's
//! own path — a kotlinc upgrade should invalidate the cache too, not
//! just source edits). If a fingerprint file from a previous run
//! matches AND the stage's declared outputs still exist on disk, the
//! stage is skipped and its outputs are reused as-is.
//!
//! The cache lives at `build/.cache/<stage-name>.hash` — inside
//! build_dir, never inside project_dir, matching this engine's
//! existing separation between generated artifacts and source
//! (see the Package & Align fix elsewhere in this crate for why that
//! separation matters).

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub struct StageCache {
    cache_dir: PathBuf,
}

impl StageCache {
    pub fn new(build_dir: &Path) -> Result<Self> {
        let cache_dir = build_dir.join(".cache");
        fs::create_dir_all(&cache_dir)?;
        Ok(StageCache { cache_dir })
    }

    /// Computes a stable fingerprint over a set of input files plus
    /// arbitrary extra strings (manifest fields, tool paths, flags —
    /// anything that isn't a file but still affects the stage's
    /// output). File order doesn't matter — paths are sorted before
    /// hashing so the fingerprint doesn't change just because
    /// `walkdir` happened to return files in a different order.
    pub fn fingerprint(input_files: &[PathBuf], extra: &[&str]) -> String {
        let mut sorted_files = input_files.to_vec();
        sorted_files.sort();

        let mut hasher = Sha256::new();
        for path in &sorted_files {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            if let Ok(contents) = fs::read(path) {
                hasher.update(&contents);
            }
            hasher.update(b"\0");
        }
        for e in extra {
            hasher.update(e.as_bytes());
            hasher.update(b"\0");
        }
        format!("{:x}", hasher.finalize())
    }

    /// Returns true if `stage_name`'s last recorded fingerprint matches
    /// `current_fingerprint` AND every path in `expected_outputs`
    /// still exists. Both conditions matter: a matching hash with a
    /// deleted output (e.g. the developer ran `cargo clean` inside
    /// their Rust project, or manually cleared `build/lib/`) must
    /// still trigger a rebuild — a cache hit promises "the output you
    /// need is right there", not just "the inputs look familiar".
    pub fn is_up_to_date(
        &self,
        stage_name: &str,
        current_fingerprint: &str,
        expected_outputs: &[PathBuf],
    ) -> bool {
        let recorded = match fs::read_to_string(self.hash_file(stage_name)) {
            Ok(s) => s,
            Err(_) => return false,
        };
        if recorded.trim() != current_fingerprint {
            return false;
        }
        expected_outputs.iter().all(|p| p.exists())
    }

    /// Records the fingerprint for a stage that just completed
    /// successfully. Never called for a failed stage — a failed
    /// stage's partial/broken output must never be mistaken for a
    /// cache hit on the next run.
    pub fn record(&self, stage_name: &str, fingerprint: &str) -> Result<()> {
        fs::write(self.hash_file(stage_name), fingerprint)?;
        Ok(())
    }

    fn hash_file(&self, stage_name: &str) -> PathBuf {
        let safe_name = stage_name.replace(['/', ' ', '[', ']'], "_");
        self.cache_dir.join(format!("{safe_name}.hash"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "youki-cache-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_cache_is_never_up_to_date() {
        let build_dir = tempdir("fresh");
        let cache = StageCache::new(&build_dir).unwrap();
        assert!(!cache.is_up_to_date("kotlin", "somehash", &[]));
    }

    #[test]
    fn matching_fingerprint_with_existing_outputs_is_a_hit() {
        let build_dir = tempdir("hit");
        let cache = StageCache::new(&build_dir).unwrap();
        let output = build_dir.join("out.class");
        fs::write(&output, "fake").unwrap();

        cache.record("kotlin", "abc123").unwrap();
        assert!(cache.is_up_to_date("kotlin", "abc123", &[output]));
    }

    #[test]
    fn changed_fingerprint_is_a_miss() {
        let build_dir = tempdir("miss");
        let cache = StageCache::new(&build_dir).unwrap();
        cache.record("kotlin", "abc123").unwrap();
        assert!(!cache.is_up_to_date("kotlin", "different-hash", &[]));
    }

    /// The critical case: hash matches, but someone deleted the actual
    /// output file behind the cache's back. Must NOT be treated as a
    /// hit — that would leave later stages (dexing, packaging) reading
    /// a file that doesn't exist.
    #[test]
    fn matching_fingerprint_with_missing_output_is_a_miss() {
        let build_dir = tempdir("missing-output");
        let cache = StageCache::new(&build_dir).unwrap();
        let output = build_dir.join("out.class"); // never created
        cache.record("kotlin", "abc123").unwrap();
        assert!(!cache.is_up_to_date("kotlin", "abc123", &[output]));
    }

    #[test]
    fn fingerprint_changes_when_file_contents_change() {
        let dir = tempdir("fp-contents");
        let file = dir.join("Main.kt");
        fs::write(&file, "fun main() {}").unwrap();
        let fp1 = StageCache::fingerprint(&[file.clone()], &[]);

        fs::write(&file, "fun main() { println(1) }").unwrap();
        let fp2 = StageCache::fingerprint(&[file], &[]);

        assert_ne!(fp1, fp2);
    }

    #[test]
    fn fingerprint_changes_when_extra_context_changes() {
        let dir = tempdir("fp-extra");
        let file = dir.join("Main.kt");
        fs::write(&file, "fun main() {}").unwrap();

        let fp1 = StageCache::fingerprint(&[file.clone()], &["minSdk=26"]);
        let fp2 = StageCache::fingerprint(&[file], &["minSdk=21"]);
        assert_ne!(fp1, fp2, "changing minSdk must invalidate the cache even with identical source");
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let dir = tempdir("fp-order");
        let a = dir.join("A.kt");
        let b = dir.join("B.kt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let fp1 = StageCache::fingerprint(&[a.clone(), b.clone()], &[]);
        let fp2 = StageCache::fingerprint(&[b, a], &[]);
        assert_eq!(fp1, fp2);
    }
}

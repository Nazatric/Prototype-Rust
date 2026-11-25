//! Block 113 — the ONE place in the crate allowed to `unsafe`: mapping
//! an archive file into memory (the house pattern from `activity` /
//! `jni_ui`; the crate-wide `forbid(unsafe_code)` stays everywhere
//! else).
//!
//! WHY THIS EXISTS: the user's phone proved the parallel index cannot
//! hold whole archives in anonymous RAM — three workers × multi-hundred-MB
//! files (art + the big cells archive + movies) tripped the low-memory
//! killer and the process died SILENTLY mid-index (four boots in the
//! journal, each stopping right after the small archives: no panic, no
//! line — the kernel killed us).  A read-only mmap hands the kernel
//! clean, evictable file-backed pages instead: under pressure it drops
//! pages, the process lives, the index finishes.
//!
//! SAFETY: `Mmap::map` on a read-only `File` is the documented safe
//! pattern (no writes, no alignment assumptions beyond what
//! `RcfArchive::parse` already makes on a `&[u8]`).

#![allow(unsafe_code)]

use memmap2::Mmap;
use std::path::Path;

/// The archive bytes: mapped (preferred) or a full read fallback.
pub(crate) enum ArchiveBytes {
    /// Read-only mapping — clean pages, evictable under pressure.
    Mapped(Mmap),
    /// Whole-file read (mmap failed on an exotic filesystem).
    Owned(Vec<u8>),
}

impl ArchiveBytes {
    /// Load the file: mmap first, fall back to a full read (the
    /// block-104 behaviour) when mapping is refused.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        match std::fs::File::open(path) {
            Ok(file) => match unsafe { Mmap::map(&file) } {
                Ok(m) => Ok(Self::Mapped(m)),
                Err(e) => {
                    log::warn!(
                        "mmap failed on {} ({e}) — falling back to a full read",
                        path.display()
                    );
                    std::fs::read(path)
                        .map(Self::Owned)
                        .map_err(|e| format!("{}: read failed: {e}", path.display()))
                }
            },
            Err(e) => Err(format!("{}: read failed: {e}", path.display())),
        }
    }

    /// The bytes (borrow lives as long as the value).
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mapped(m) => &m[..],
            Self::Owned(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mmap path reads a real fixture correctly (same bytes a
    /// plain read returns).
    #[test]
    fn mapped_bytes_match_a_plain_read() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("scripts.rcf");
        let mapped = ArchiveBytes::load(&path).expect("load");
        let plain = std::fs::read(&path).expect("read");
        assert_eq!(mapped.as_slice(), plain.as_slice());
    }
}

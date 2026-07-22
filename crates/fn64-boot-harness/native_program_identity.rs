//! Canonical build-time identity for linked native program archives.

use sha2::{Digest, Sha256};

/// Hash the exact bytes of a native program's produced archives in canonical
/// logical-label order.
///
/// Filesystem paths and host pointers never enter this wire. Archive-internal
/// bytes are deliberately retained: if a compiler, archiver, target, build
/// policy, member order, or embedded metadata changes the linked artifact, its
/// identity must change too. Duplicate labels fail loudly because accepting one
/// would make the claimed archive set ambiguous.
pub fn native_program_archives_sha256(
    entries: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> [u8; 32] {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    for pair in entries.windows(2) {
        assert_ne!(
            pair[0].0, pair[1].0,
            "native program archive set repeats logical label {:?}",
            pair[0].0
        );
    }

    let mut digest = Sha256::new();
    digest.update(b"fn64.native-program-archives.v1\0");
    digest.update((entries.len() as u64).to_be_bytes());
    for (label, bytes) in entries {
        digest.update((label.len() as u64).to_be_bytes());
        digest.update(label.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_order_independent_but_collision_sensitive() {
        let first = native_program_archives_sha256([
            ("generated-code".to_owned(), b"archive-a".to_vec()),
            ("section-bridge".to_owned(), b"archive-b".to_vec()),
        ]);
        let reordered = native_program_archives_sha256([
            ("section-bridge".to_owned(), b"archive-b".to_vec()),
            ("generated-code".to_owned(), b"archive-a".to_vec()),
        ]);
        let changed_bytes = native_program_archives_sha256([
            ("generated-code".to_owned(), b"changed".to_vec()),
            ("section-bridge".to_owned(), b"archive-b".to_vec()),
        ]);
        let changed_label = native_program_archives_sha256([
            ("generated-code".to_owned(), b"archive-a".to_vec()),
            ("other-bridge".to_owned(), b"archive-b".to_vec()),
        ]);

        assert_eq!(first, reordered);
        assert_ne!(first, changed_bytes);
        assert_ne!(first, changed_label);
    }

    #[test]
    #[should_panic(expected = "repeats logical label")]
    fn archives_reject_duplicate_labels() {
        let _ = native_program_archives_sha256([
            ("generated-code".to_owned(), b"archive-a".to_vec()),
            ("generated-code".to_owned(), b"archive-b".to_vec()),
        ]);
    }
}

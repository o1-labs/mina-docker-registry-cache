//! Pure retention logic — no I/O, fully unit-testable.
//!
//! Policy: "keep last N per repository". For each repository we keep the `N`
//! most recently *pushed* tags and mark the rest for deletion. Recency is the
//! modification time of the tag's `current/link` file on disk, which the
//! registry rewrites every time a tag is (re)pushed.

use std::time::SystemTime;

/// A single tag of a repository as seen on the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Manifest digest the tag points at, e.g. `sha256:abc…`. Used for logging.
    pub digest: String,
    /// Modification time of `.../tags/<name>/current/link` — our recency signal.
    pub mtime: SystemTime,
}

/// Given all tags of a repository, return the ones to delete: everything beyond
/// the `keep` most-recent. Newest is decided by `mtime` descending; ties are
/// broken by tag name (descending) so the result is deterministic regardless of
/// directory-iteration order.
pub fn tags_to_delete(tags: &[Tag], keep: usize) -> Vec<Tag> {
    if tags.len() <= keep {
        return Vec::new();
    }
    let mut sorted = tags.to_vec();
    // Most-recent first. On equal mtime, larger name sorts first — arbitrary but
    // stable, which is all we need for reproducible pruning.
    sorted.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| b.name.cmp(&a.name)));
    sorted.split_off(keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tag(name: &str, secs: u64) -> Tag {
        Tag {
            name: name.to_string(),
            digest: format!("sha256:{name}"),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn keeps_newest_n_by_mtime() {
        let tags = vec![
            tag("v1", 100),
            tag("v2", 200),
            tag("v3", 300),
            tag("v4", 400),
        ];
        let del = tags_to_delete(&tags, 2);
        let names: Vec<_> = del.iter().map(|t| t.name.as_str()).collect();
        // v3 (300) and v4 (400) are newest -> v1, v2 deleted.
        assert_eq!(names, vec!["v2", "v1"]);
    }

    #[test]
    fn keep_zero_deletes_everything() {
        let tags = vec![tag("a", 1), tag("b", 2)];
        assert_eq!(tags_to_delete(&tags, 0).len(), 2);
    }

    #[test]
    fn keep_at_or_above_count_deletes_nothing() {
        let tags = vec![tag("a", 1), tag("b", 2)];
        assert!(tags_to_delete(&tags, 2).is_empty());
        assert!(tags_to_delete(&tags, 5).is_empty());
        assert!(tags_to_delete(&[], 3).is_empty());
    }

    #[test]
    fn equal_mtime_is_deterministic_by_name() {
        // All same mtime: tie-break keeps the lexicographically-largest names.
        let tags = vec![tag("a", 50), tag("b", 50), tag("c", 50)];
        let del = tags_to_delete(&tags, 1);
        let names: Vec<_> = del.iter().map(|t| t.name.as_str()).collect();
        // "c" is kept (largest), "b" then "a" deleted.
        assert_eq!(names, vec!["b", "a"]);
    }

    #[test]
    fn input_order_does_not_change_result() {
        let a = vec![tag("v1", 100), tag("v2", 200), tag("v3", 300)];
        let b = vec![tag("v3", 300), tag("v1", 100), tag("v2", 200)];
        let da: Vec<_> = tags_to_delete(&a, 1).into_iter().map(|t| t.name).collect();
        let db: Vec<_> = tags_to_delete(&b, 1).into_iter().map(|t| t.name).collect();
        assert_eq!(da, db);
    }
}

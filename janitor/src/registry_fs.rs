//! Filesystem view of a `registry:2` filesystem-driver storage tree.
//!
//! Layout (under `<rootdirectory>/docker/registry/v2`):
//!
//! ```text
//! repositories/<name>/_manifests/tags/<tag>/current/link   # text: "sha256:…"
//! repositories/<name>/_manifests/revisions/sha256/<hex>/…
//! blobs/sha256/<2>/<hex>/data
//! ```
//!
//! Repository names can be nested (`mina/daemon`), so discovery walks the tree
//! and treats any directory containing a `_manifests` child as a repository.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::retention::Tag;

pub struct Layout {
    repositories: PathBuf,
}

impl Layout {
    /// `data_dir` is the registry `rootdirectory` (e.g. `/var/lib/registry`).
    pub fn new(data_dir: &Path) -> Self {
        Layout {
            repositories: data_dir.join("docker/registry/v2/repositories"),
        }
    }

    /// All repository names found under the storage tree, sorted.
    pub fn repositories(&self) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        if self.repositories.is_dir() {
            walk(&self.repositories, &self.repositories, &mut out)?;
        }
        out.sort();
        Ok(out)
    }

    /// Tags of a repository, each with its manifest digest and `current/link`
    /// mtime. Tags without a readable `current/link` are skipped.
    pub fn tags(&self, repo: &str) -> io::Result<Vec<Tag>> {
        let tags_dir = self.repositories.join(repo).join("_manifests/tags");
        let mut out = Vec::new();
        if !tags_dir.is_dir() {
            return Ok(out);
        }
        for entry in fs::read_dir(&tags_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let link = entry.path().join("current/link");
            let (digest, mtime) = match (fs::read_to_string(&link), fs::metadata(&link)) {
                (Ok(content), Ok(meta)) => match meta.modified() {
                    Ok(m) => (content.trim().to_string(), m),
                    Err(_) => continue,
                },
                _ => continue, // tag dir with no current/link — ignore
            };
            out.push(Tag {
                name: entry.file_name().to_string_lossy().into_owned(),
                digest,
                mtime,
            });
        }
        Ok(out)
    }

    /// Remove a tag by deleting its `tags/<tag>` directory. The underlying
    /// manifest revision (and its blobs) is reclaimed later by `garbage-collect`.
    pub fn delete_tag(&self, repo: &str, tag: &str) -> io::Result<()> {
        let dir = self
            .repositories
            .join(repo)
            .join("_manifests/tags")
            .join(tag);
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Depth-first walk: a directory holding `_manifests` is a repository; otherwise
/// descend into its subdirectories. We never descend *into* a repository, so a
/// tag literally named like a path component can't be mistaken for a repo.
fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    if dir.join("_manifests").is_dir() {
        if let Ok(rel) = dir.strip_prefix(base) {
            // Normalise to forward slashes for registry-style names.
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            walk(base, &entry.path(), out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// Build a minimal storage tree in a temp dir and return its root.
    fn scaffold(root: &Path, repo: &str, tag: &str, digest: &str) {
        let tagdir = root
            .join("docker/registry/v2/repositories")
            .join(repo)
            .join("_manifests/tags")
            .join(tag)
            .join("current");
        fs::create_dir_all(&tagdir).unwrap();
        fs::write(tagdir.join("link"), digest).unwrap();
    }

    fn tmpdir() -> PathBuf {
        // Deterministic, collision-resistant temp path without external crates.
        let base = std::env::temp_dir();
        let pid = std::process::id();
        let uniq = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = base.join(format!("janitor-test-{pid}-{uniq}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn discovers_nested_repositories() {
        let root = tmpdir();
        scaffold(&root, "alpine", "v1", "sha256:a");
        scaffold(&root, "mina/daemon", "v1", "sha256:b");
        scaffold(&root, "mina/archive", "v1", "sha256:c");

        let layout = Layout::new(&root);
        let mut repos = layout.repositories().unwrap();
        repos.sort();
        assert_eq!(repos, vec!["alpine", "mina/archive", "mina/daemon"]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_tags_with_digests() {
        let root = tmpdir();
        scaffold(&root, "app", "v1", "sha256:111");
        scaffold(&root, "app", "v2", "sha256:222");

        let layout = Layout::new(&root);
        let mut tags = layout.tags("app").unwrap();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "v1");
        assert_eq!(tags[0].digest, "sha256:111");
        assert_eq!(tags[1].digest, "sha256:222");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_tag_removes_dir_and_is_idempotent() {
        let root = tmpdir();
        scaffold(&root, "app", "v1", "sha256:1");
        let layout = Layout::new(&root);

        assert_eq!(layout.tags("app").unwrap().len(), 1);
        layout.delete_tag("app", "v1").unwrap();
        assert_eq!(layout.tags("app").unwrap().len(), 0);
        // Deleting again must not error.
        layout.delete_tag("app", "v1").unwrap();

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tag_without_current_link_is_skipped() {
        let root = tmpdir();
        let bad = root.join("docker/registry/v2/repositories/app/_manifests/tags/broken");
        fs::create_dir_all(&bad).unwrap();
        scaffold(&root, "app", "good", "sha256:ok");

        let layout = Layout::new(&root);
        let tags = layout.tags("app").unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "good");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mtime_increases_with_write_order() {
        let root = tmpdir();
        scaffold(&root, "app", "old", "sha256:o");
        std::thread::sleep(Duration::from_millis(1100));
        scaffold(&root, "app", "new", "sha256:n");

        let layout = Layout::new(&root);
        let tags = layout.tags("app").unwrap();
        let old = tags.iter().find(|t| t.name == "old").unwrap();
        let new = tags.iter().find(|t| t.name == "new").unwrap();
        assert!(new.mtime >= old.mtime);

        fs::remove_dir_all(&root).ok();
    }
}

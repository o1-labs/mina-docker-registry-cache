//! Thin wrapper around the registry's own garbage collector.
//!
//! We never reimplement mark-and-sweep — that is where it is easy to corrupt a
//! store (e.g. dropping a child manifest of a multi-arch index). Instead the
//! janitor only removes *tags* on the filesystem, then delegates blob/manifest
//! reclamation to `registry garbage-collect`, which understands manifest lists.

use std::io;
use std::process::Command;

/// Run `registry garbage-collect [--delete-untagged] <config>`.
///
/// `--delete-untagged` is what actually reclaims the manifest revisions our tag
/// deletions just orphaned (and their blobs). Recent registry versions handle
/// manifest lists correctly here; the flag is exposed so it can be disabled on
/// older registries if needed.
pub fn run_gc(registry_bin: &str, config_path: &str, delete_untagged: bool) -> io::Result<bool> {
    let mut cmd = Command::new(registry_bin);
    cmd.arg("garbage-collect");
    if delete_untagged {
        cmd.arg("--delete-untagged");
    }
    cmd.arg(config_path);
    Ok(cmd.status()?.success())
}

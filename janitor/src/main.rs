//! mina-docker-registry-cache janitor
//!
//! Enforces a "keep last N tags per repository" retention policy on a
//! `registry:2` filesystem store, then runs the registry's garbage collector to
//! reclaim disk. Designed to run as a sidecar that shares the registry's
//! storage volume (e.g. a Hetzner storage box mounted as a local filesystem).
//!
//! Configuration (environment variables):
//!   JANITOR_DATA_DIR         registry rootdirectory      (default /var/lib/registry)
//!   JANITOR_REGISTRY_CONFIG  registry config for GC      (default /etc/docker/registry/config.yml)
//!   JANITOR_REGISTRY_BIN     registry binary name/path   (default registry)
//!   KEEP_LAST_N              tags kept per repository     (default 10)
//!   JANITOR_INTERVAL_SECS    sleep between sweeps         (default 3600)
//!   GC_DELETE_UNTAGGED       pass --delete-untagged       (default true)
//!   RUN_GC                   run GC after pruning         (default true)
//!   RUN_ONCE                 one sweep then exit          (default false)
//!   DRY_RUN                  log only, change nothing     (default false)
//!
//! Note: janitor vars are NOT prefixed `REGISTRY_` on purpose — the registry
//! binary treats `REGISTRY_*` as its own config namespace and would warn about
//! ours when the janitor shells out to `registry garbage-collect`.

mod gc;
mod registry_fs;
mod retention;

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use registry_fs::Layout;

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn log(msg: &str) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("[janitor t={t}] {msg}");
}

struct Config {
    data_dir: PathBuf,
    registry_config: String,
    registry_bin: String,
    keep: usize,
    interval: u64,
    gc_delete_untagged: bool,
    run_gc: bool,
    run_once: bool,
    dry_run: bool,
}

impl Config {
    fn from_env() -> Self {
        Config {
            data_dir: PathBuf::from(env_str("JANITOR_DATA_DIR", "/var/lib/registry")),
            registry_config: env_str("JANITOR_REGISTRY_CONFIG", "/etc/docker/registry/config.yml"),
            registry_bin: env_str("JANITOR_REGISTRY_BIN", "registry"),
            keep: env_usize("KEEP_LAST_N", 10),
            interval: env_u64("JANITOR_INTERVAL_SECS", 3600),
            gc_delete_untagged: env_bool("GC_DELETE_UNTAGGED", true),
            run_gc: env_bool("RUN_GC", true),
            run_once: env_bool("RUN_ONCE", false),
            dry_run: env_bool("DRY_RUN", false),
        }
    }
}

/// One retention sweep. Returns the number of tags deleted (or that *would* be
/// deleted under DRY_RUN).
fn sweep(layout: &Layout, cfg: &Config) -> std::io::Result<usize> {
    let repos = layout.repositories()?;
    log(&format!(
        "scanning {} repositories (keep_last_n={})",
        repos.len(),
        cfg.keep
    ));

    let mut deleted = 0usize;
    for repo in &repos {
        let tags = layout.tags(repo)?;
        let to_delete = retention::tags_to_delete(&tags, cfg.keep);
        if to_delete.is_empty() {
            continue;
        }
        log(&format!(
            "repo '{repo}': {} tags -> deleting {}",
            tags.len(),
            to_delete.len()
        ));
        for t in &to_delete {
            log(&format!(
                "  - {}delete {repo}:{} ({})",
                if cfg.dry_run { "[dry-run] " } else { "" },
                t.name,
                t.digest
            ));
            if !cfg.dry_run {
                layout.delete_tag(repo, &t.name)?;
            }
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn main() {
    let cfg = Config::from_env();
    log(&format!(
        "starting: data_dir={} keep_last_n={} interval={}s gc={} delete_untagged={} run_once={} dry_run={}",
        cfg.data_dir.display(),
        cfg.keep,
        cfg.interval,
        cfg.run_gc,
        cfg.gc_delete_untagged,
        cfg.run_once,
        cfg.dry_run,
    ));

    let layout = Layout::new(&cfg.data_dir);

    loop {
        match sweep(&layout, &cfg) {
            Ok(n) => {
                log(&format!("pruned {n} tags"));
                if n > 0 && cfg.run_gc && !cfg.dry_run {
                    match gc::run_gc(
                        &cfg.registry_bin,
                        &cfg.registry_config,
                        cfg.gc_delete_untagged,
                    ) {
                        Ok(true) => log("garbage-collect: ok"),
                        Ok(false) => log("garbage-collect: non-zero exit"),
                        Err(e) => log(&format!("garbage-collect: failed to run: {e}")),
                    }
                }
            }
            Err(e) => log(&format!("sweep error: {e}")),
        }

        if cfg.run_once {
            break;
        }
        thread::sleep(Duration::from_secs(cfg.interval));
    }
}

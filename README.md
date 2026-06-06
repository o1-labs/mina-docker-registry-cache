# mina-docker-registry-cache

A cheap, self-hosted Docker registry for **short-lived CI images**, backed by a
**Hetzner storage box** mounted as a local filesystem. A drop-in alternative to
pushing ephemeral CI images to GCR / Google Artifact Registry, where egress and
storage are expensive.

It is deliberately **not** a from-scratch registry. The Docker Registry v2 / OCI
distribution protocol (chunked uploads, manifest content-type negotiation, tag
pagination, range requests, GC) is large and easy to get subtly wrong. So:

| Concern                         | How                                                          |
| ------------------------------- | ----------------------------------------------------------- |
| The registry protocol           | Stock [`registry:2`](https://hub.docker.com/_/registry) (CNCF Distribution), filesystem driver |
| Storage backend                 | A directory on the Hetzner storage box, mounted as local FS |
| Retention (short-lived cleanup) | A small **Rust janitor** sidecar — *keep last N tags per repo* |
| Disk reclamation                | `registry garbage-collect`, invoked by the janitor          |
| Auth                            | None — intended for a trusted/private CI network            |

```
        docker push/pull                 shared storage volume
CI  ─────────────────────►  registry:2  ◄────────────────────►  janitor (Rust)
                            (protocol)     /var/lib/registry      prune tags
                                           on storage box         + garbage-collect
```

## Why a janitor at all?

`registry:2` has **no automatic expiry**. Left alone, a CI cache grows forever.
The janitor enforces a rolling window:

1. Walk the storage tree, discover every repository (nested names like
   `mina/daemon` included).
2. For each repo, rank tags by **push time** (mtime of the tag's
   `current/link`) and delete everything beyond the newest `KEEP_LAST_N`.
   This is a pure filesystem operation — no API races.
3. Run `registry garbage-collect --delete-untagged` to reclaim the now-orphaned
   manifest revisions and their blobs. GC (not the janitor) owns blob
   reclamation, so manifest lists / multi-arch images stay intact.

The janitor binary is baked **into** the registry image (same base), so it can
call `registry garbage-collect` directly — no Docker socket, no second runtime.

## Quick start (local)

```bash
cp .env.example .env          # optional: tweak KEEP_LAST_N, port, interval
make build
make up                       # registry on :5000, janitor looping hourly

# use it
docker pull busybox
docker tag busybox localhost:5000/team/app:abc123
docker push localhost:5000/team/app:abc123
docker pull localhost:5000/team/app:abc123

# prune on demand instead of waiting for the interval
make gc                       # one prune + garbage-collect pass, then exits
```

## Production: storage box as backend

The storage box is expected to be **mounted on the host** as a local filesystem.
CIFS/SMB is recommended — it supports the atomic rename the filesystem driver
relies on. (Avoid SSHFS for the live store: weaker rename/locking semantics.)

`/etc/fstab` example:

```
//u4XXXXX.your-storagebox.de/backup  /mnt/storagebox  cifs  credentials=/etc/storagebox.cred,uid=0,gid=0,iocharset=utf8,_netdev  0 0
```

Then point both services at it via an override:

```bash
cp docker-compose.override.yml.example docker-compose.override.yml
# edit `source:` to your mount path, e.g. /mnt/storagebox/registry
make up
```

Compose automatically merges `docker-compose.override.yml`, replacing the
default named volume with the bind mount.

## Configuration

Set in `.env` (or the environment). Defaults in parentheses.

| Variable                | Default                  | Meaning                                        |
| ----------------------- | ------------------------ | ---------------------------------------------- |
| `REGISTRY_PORT`         | `5000`                   | Host port for the registry                     |
| `KEEP_LAST_N`           | `10`                     | Tags kept per repository (newest by push time) |
| `JANITOR_INTERVAL_SECS` | `3600`                   | Seconds between janitor sweeps                  |
| `GC_DELETE_UNTAGGED`    | `true`                   | Pass `--delete-untagged` to GC                  |
| `RUN_ONCE`              | `false`                  | One sweep then exit (used by `make gc`)         |
| `DRY_RUN`               | `false`                  | Log what would be deleted, change nothing       |

Janitor-only (rarely changed): `JANITOR_DATA_DIR` (`/var/lib/registry`),
`JANITOR_REGISTRY_CONFIG` (`/etc/docker/registry/config.yml`),
`JANITOR_REGISTRY_BIN` (`registry`), `RUN_GC` (`true`). These intentionally
avoid the `REGISTRY_` prefix, which the registry binary reserves for its own
config overrides.

## Tests

```bash
make test-unit          # Rust: retention selection + FS scanning (no Docker)
make test-integration   # spins up the stack, pushes 4 images, prunes to 2,
                        # asserts the right tags survive and pull/fail correctly
make test               # both
```

CI runs both on every push/PR (`.github/workflows/ci.yml`).

## Releases (GHCR)

Pushing a version tag publishes the janitor image to GitHub Container Registry
(`.github/workflows/release.yml`, gated on the tests passing):

```bash
git tag v0.1.0 && git push origin v0.1.0
# -> ghcr.io/o1-labs/mina-docker-registry-cache/janitor:0.1.0
#    ghcr.io/o1-labs/mina-docker-registry-cache/janitor:0.1
#    ghcr.io/o1-labs/mina-docker-registry-cache/janitor:latest
```

The registry itself is stock `registry:2.8.3` (not republished). To use the
published janitor instead of building locally, set the janitor service's image
to the GHCR tag and drop its `build:` line. First publish may require the
package to be linked to the repo / made visible in the org's package settings.

## Operational notes

- **Retention is per repository.** `team/app` and `team/api` each keep their own
  newest `KEEP_LAST_N` tags.
- **Recency = last push**, not image build time. Re-pushing a tag refreshes it.
- **GC concurrency.** `registry garbage-collect` can, in principle, race a blob
  that was just uploaded for an in-flight push and not yet referenced by a
  manifest. For a CI cache this window is tiny; schedule the janitor for
  low-traffic periods (or lengthen `JANITOR_INTERVAL_SECS`) if your CI pushes
  continuously. Set the registry to read-only during GC for zero risk.
- **No auth by design.** Keep it on a private network / firewalled / VPN. To add
  basic auth later, mount an `htpasswd` file and set `REGISTRY_AUTH=htpasswd`
  on the registry service — the janitor needs no auth (it works on the FS).
- **Pinned to `registry:2.8.3`** for predictable GC behavior; `registry:3.x`
  also works.

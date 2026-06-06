#!/usr/bin/env bash
# End-to-end test: push N distinct images, run the janitor with KEEP_LAST_N=2,
# and assert that only the two newest tags survive (and are still pullable),
# while the older tags are gone and the registry garbage-collects cleanly.
#
# Requires Docker + docker compose. Uses an isolated compose project and a
# throwaway port so it won't collide with a running instance.
set -euo pipefail

cd "$(dirname "$0")/.."

PORT="${TEST_REGISTRY_PORT:-5555}"
REG="localhost:${PORT}"
REPO="${REG}/citest/app"
PROJECT="mina-docker-registry-cache-test"
COMPOSE=(docker compose -p "$PROJECT")

pass() { printf '  \033[32mPASS\033[0m %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; exit 1; }

cleanup() {
  REGISTRY_PORT="$PORT" "${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
  for n in 1 2 3 4; do docker rmi "${REPO}:v${n}" >/dev/null 2>&1 || true; done
}
trap cleanup EXIT

echo "==> building images"
REGISTRY_PORT="$PORT" "${COMPOSE[@]}" build

echo "==> starting registry"
REGISTRY_PORT="$PORT" "${COMPOSE[@]}" up -d registry

echo "==> waiting for registry to be ready"
ready=""
for _ in $(seq 1 30); do
  if curl -fsS "http://${REG}/v2/" >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[ -n "$ready" ] || fail "registry did not become ready"
pass "registry is serving /v2/"

echo "==> pushing 4 distinct images (v1..v4)"
docker pull busybox:latest >/dev/null
for n in 1 2 3 4; do
  # Each image is unique (different file) so it has its own manifest digest,
  # which exercises real garbage collection of pruned revisions.
  printf 'FROM busybox:latest\nRUN echo "build-%s" > /buildid\n' "$n" \
    | docker build -q -t "${REPO}:v${n}" - >/dev/null
  docker push "${REPO}:v${n}" >/dev/null
  sleep 1  # ensure distinct current/link mtimes for recency ordering
done

before="$(curl -fsS "http://${REG}/v2/citest/app/tags/list")"
echo "    tags before: $before"
for n in 1 2 3 4; do
  echo "$before" | grep -q "\"v${n}\"" || fail "expected v${n} present before prune"
done
pass "all 4 tags pushed"

echo "==> running janitor once (KEEP_LAST_N=2)"
REGISTRY_PORT="$PORT" "${COMPOSE[@]}" run --rm \
  -e KEEP_LAST_N=2 -e RUN_ONCE=true -e GC_DELETE_UNTAGGED=true janitor

after="$(curl -fsS "http://${REG}/v2/citest/app/tags/list")"
echo "    tags after:  $after"

echo "==> assertions"
echo "$after" | grep -q '"v4"' || fail "v4 (newest) should remain"
echo "$after" | grep -q '"v3"' || fail "v3 (newest) should remain"
pass "newest two tags (v3, v4) retained"

echo "$after" | grep -q '"v1"' && fail "v1 should have been pruned"
echo "$after" | grep -q '"v2"' && fail "v2 should have been pruned"
pass "oldest two tags (v1, v2) pruned"

docker rmi "${REPO}:v1" >/dev/null 2>&1 || true
if docker pull "${REPO}:v1" >/dev/null 2>&1; then
  fail "pruned tag v1 is still pullable"
fi
pass "pruned tag v1 is no longer pullable"

docker rmi "${REPO}:v4" >/dev/null 2>&1 || true
docker pull "${REPO}:v4" >/dev/null 2>&1 || fail "retained tag v4 should still pull"
pass "retained tag v4 still pulls"

echo
echo "ALL INTEGRATION TESTS PASSED"

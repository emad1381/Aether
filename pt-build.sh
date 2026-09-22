#!/usr/bin/env bash
set -euo pipefail

goos="${1:-}"
goarch="${2:-}"
out="${3:-}"
goarm="${4:-}"

if [ -z "$goos" ] || [ -z "$goarch" ] || [ -z "$out" ]; then
    echo "usage: pt-build.sh <goos> <goarch> <outdir> [goarm]" >&2
    exit 2
fi

module="gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird"
repo="https://$module.git"
commit="fc105a03c0e0"
version="v0.0.0-20260312101154-fc105a03c0e0"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

src=""
if git clone --quiet "$repo" "$work/lyrebird" 2>/dev/null; then
    src="$work/lyrebird"
    git -C "$src" checkout --quiet "$commit" 2>/dev/null ||
        echo "pt-build: pinned commit is gone, using the default branch" >&2
else
    echo "pt-build: cannot reach gitlab, taking the go module proxy instead" >&2
    curl -fsSL "https://proxy.golang.org/$module/@v/$version.zip" -o "$work/lyrebird.zip"
    (cd "$work" && unzip -q lyrebird.zip)

    src="$work/$module@$version"
    if [ ! -f "$src/go.mod" ]; then
        src="$(dirname "$(find "$work" -type f -name go.mod | head -n 1)")"
    fi
    chmod -R u+w "$src"
fi

if [ ! -f "$src/go.mod" ]; then
    echo "pt-build: no lyrebird source to build" >&2
    exit 1
fi

ext=""
if [ "$goos" = "windows" ]; then
    ext=".exe"
fi

mkdir -p "$out"

(
    cd "$src"
    GOOS="$goos" GOARCH="$goarch" GOARM="$goarm" CGO_ENABLED=0 \
        go build -trimpath -ldflags "-s -w" -o "$out/lyrebird$ext" ./cmd/lyrebird
)

# snowflake-client: the snowflake pluggable transport. Nice to have, not
# critical: obfs4 and webtunnel ship in lyrebird, so a failed snowflake build
# only costs one fallback lane, and must never break the release build.
sf_repo="https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake"
sf_version="v2.6.1"
sf_module="gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake"

sf_work="$(mktemp -d)"
if git clone --quiet --depth 1 --branch "$sf_version" "$sf_repo" "$sf_work/snowflake" 2>/dev/null; then
    sf_src="$sf_work/snowflake"
else
    curl -fsSL "https://proxy.golang.org/$sf_module/@v/$sf_version.zip" -o "$sf_work/snowflake.zip" 2>/dev/null || true
    if [ -f "$sf_work/snowflake.zip" ]; then
        (cd "$sf_work" && unzip -q snowflake.zip)
        sf_src="$sf_work/$sf_module@$sf_version"
    fi
fi

if [ -n "${sf_src:-}" ] && [ -f "$sf_src/client/main.go" ]; then
    if (
        cd "$sf_src"
        GOOS="$goos" GOARCH="$goarch" GOARM="$goarm" CGO_ENABLED=0 \
            go build -trimpath -ldflags "-s -w" -o "$out/snowflake-client$ext" ./client
    ); then
        echo "pt-build: snowflake-client built"
    else
        echo "pt-build: snowflake-client failed to build; continuing without it" >&2
    fi
else
    echo "pt-build: snowflake source unavailable; continuing without it" >&2
fi
rm -rf "$sf_work"

echo "pt-build: $goos/$goarch ->"
ls -l "$out"

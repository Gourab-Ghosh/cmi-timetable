#!/usr/bin/env bash
# Deploy the app to GitHub Pages from THIS machine: the whole build runs
# here, so a GitHub Actions outage cannot fail a release. (Serving the
# branch is still GitHub's own step — see verify_published below.)
#
#   ./deploy.sh                  # test + build + publish + verify
#   ./deploy.sh --sync           # refresh the CMI data mirror first
#   ./deploy.sh --push           # also push main first (ship code + site)
#   ./deploy.sh --skip-tests     # publish without running the test suite
#   ./deploy.sh --allow-stale    # publish even if origin/main is ahead
#   ./deploy.sh --no-verify      # don't wait for GitHub to serve the build
#   ./deploy.sh --republish      # re-trigger serving of what's already there
#   ./deploy.sh --build-only     # rehearse: build (and test), publish nothing
#
# Nothing about the release runs on GitHub's infrastructure: there are no
# workflows in this repo, so no CI job can fail, stall or send failure mail.
# `--sync` does locally what the old cron did — fetch both CMI pages, run
# them through the same parser and validation gate, and update the mirror
# under app/public/data (committed as data, never as build output).
#
# How it works:
#   1. Builds the app with Trunk inside a temporary Docker container
#      (rust:1); if Docker is unavailable, builds locally instead.
#   2. Publishes the built site as a SINGLE orphan commit on the `gh-pages`
#      branch (built in a scratch index, so the working tree is untouched)
#      and force-pushes it. `main` never carries build artifacts, and the
#      branch keeps no history — repo visitors only ever see source.
#   3. GitHub Pages (set to serve the `gh-pages` branch) picks it up in a
#      minute or two. Serving is the one step GitHub still owns — it copies
#      static files, no toolchain involved — so the script then checks that
#      the live site really shows this build and asks for a rebuild if not.
#
# Caches live in .build-cache/ (gitignored) inside the repo, so repeat runs
# are fast and nothing is written outside this folder.
#
# Env knobs: PUBLIC_URL (default "/<repo-name>/"; use "/" for a
# user/organization page), TRUNK_VERSION, FORCE_LOCAL=1 (skip Docker),
# DOCKER_IMAGE, CARGO_TARGET_DIR (local builds only).

set -euo pipefail
cd "$(dirname "$0")"

TRUNK_VERSION="${TRUNK_VERSION:-v0.21.14}"
DOCKER_IMAGE="${DOCKER_IMAGE:-rust:1}"
CACHE="$PWD/.build-cache"
DIST="app/dist-deploy"
BRANCH="gh-pages"

SKIP_TESTS=0
ALLOW_STALE=0
PUSH_MAIN=0
VERIFY=1
REPUBLISH=0
SYNC_DATA=0
BUILD_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --skip-tests)  SKIP_TESTS=1 ;;
        --allow-stale) ALLOW_STALE=1 ;;
        --push)        PUSH_MAIN=1 ;;
        --no-verify)   VERIFY=0 ;;
        --republish)   REPUBLISH=1 ;;
        --sync)        SYNC_DATA=1 ;;
        --build-only)  BUILD_ONLY=1 ;;
        # Print the whole header comment, however long it grows.
        -h|--help) awk 'NR>1 { if (!/^#/) exit; print }' "$0"; exit 0 ;;
        *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
    esac
done

command -v git >/dev/null || { echo "git is required" >&2; exit 1; }
if [ "$PUSH_MAIN" = 1 ]; then
    echo "==> pushing $(git rev-parse --abbrev-ref HEAD) to origin"
    git push origin HEAD
fi
ORIGIN=$(git remote get-url origin)
REPO_NAME=$(basename -s .git "$ORIGIN")
PUBLIC_URL="${PUBLIC_URL:-/$REPO_NAME/}"
SHA=$(git rev-parse --short HEAD)
DIRTY=""
[ -z "$(git status --porcelain)" ] || DIRTY="+dirty"

# owner/repo and the public site URL, when origin is a GitHub remote.
# (Bash regexes are POSIX ERE — no lazy quantifiers — so take the repo name
# from basename -s .git rather than trying to strip .git in the pattern.)
SLUG=""
SITE=""
if [[ "$ORIGIN" =~ github\.com[:/]([^/]+)/ ]]; then
    owner="${BASH_REMATCH[1]}"
    SLUG="$owner/$REPO_NAME"
    SITE="https://$(echo "$owner" | tr '[:upper:]' '[:lower:]').github.io$PUBLIC_URL"
fi

# What GitHub reports about the last Pages build, for messages only — never
# as the success signal: right after a push the "latest" build is still the
# previous one, so its status says nothing about this deploy.
pages_status() {
    if [ -n "$SLUG" ] && command -v gh >/dev/null; then
        gh api "repos/$SLUG/pages/builds/latest" --jq '.status' 2>/dev/null \
            || echo "unavailable (Pages API returned nothing)"
    else
        echo "unknown (no gh CLI)"
    fi
}

request_pages_rebuild() {
    [ -n "$SLUG" ] && command -v gh >/dev/null || return 0
    gh api -X POST "repos/$SLUG/pages/builds" >/dev/null 2>&1 || true
}

# Serving the branch is GitHub's job and it can stall or fail during their
# outages. The ground truth is the live page itself: poll it for the build we
# just published, and if it doesn't show up, ask Pages to rebuild and poll
# once more. Our artifact is on the branch either way, so this never fails
# the deploy — it only tells the truth about it.
verify_published() {
    local expect="$1" attempt=1 waited status
    if [ -z "$expect" ]; then
        echo "note: could not fingerprint the build — skipping live verification"
        return 0
    fi
    [ -n "$SITE" ] || { echo "note: not a GitHub remote — skipping live verification"; return 0; }
    echo "==> waiting for $SITE to serve it"
    while :; do
        waited=0
        while [ "$waited" -lt 180 ]; do   # ~3 min per attempt
            # Cache-buster: Pages sits behind a CDN, and a cached copy of the
            # old page would make this check lie in both directions.
            if curl -fsSL "${SITE}?deploy-check=$waited.$attempt" 2>/dev/null | grep -q "$expect"; then
                echo "==> live: $SITE is serving this build"
                return 0
            fi
            sleep 15
            waited=$((waited + 15))
        done
        status=$(pages_status)
        [ "$attempt" = 1 ] || break
        echo "!! not served after 3 min (pages build: $status) — asking for a rebuild"
        request_pages_rebuild
        attempt=2
    done
    echo "!! the build IS published on $BRANCH, but GitHub is not serving it yet"
    echo "   (pages build: $status). That step is theirs, not the build's."
    echo "   Check https://www.githubstatus.com, then: ./deploy.sh --republish"
    return 0
}

# Filename of the content-hashed wasm in a directory — the fingerprint the
# published index.html must reference for the site to be serving this build.
dist_fingerprint() {
    local f
    for f in "$1"/*_bg.wasm; do
        [ -e "$f" ] || return 1
        basename "$f"
        return 0
    done
}

# --republish: nudge GitHub into serving what is already on the branch, with
# no rebuild. Re-pointing the branch at a fresh commit with the SAME tree is
# a push event, which is what triggers Pages — so this works even when the
# Pages API itself is unreachable.
if [ "$REPUBLISH" = 1 ]; then
    [ -n "$SLUG" ] || { echo "--republish needs a GitHub origin" >&2; exit 1; }
    git fetch -q origin "$BRANCH"
    TREE=$(git rev-parse "origin/$BRANCH^{tree}")
    export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-$(git config user.name || echo deploy)}"
    export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-$(git config user.email || echo deploy@localhost)}"
    export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
    export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
    COMMIT=$(git commit-tree "$TREE" -m "republish $(date -u +%Y-%m-%dT%H:%M:%SZ)")
    echo "==> re-pushing $BRANCH (same content) to trigger serving"
    git push -q --force "$ORIGIN" "$COMMIT:refs/heads/$BRANCH"
    request_pages_rebuild
    if [ "$VERIFY" = 1 ]; then
        # No match is normal-ish (someone else published the branch), not a
        # reason to abort after a successful push — hence the `|| true`.
        expect=$(git show "$COMMIT:index.html" 2>/dev/null \
            | grep -o '[A-Za-z0-9_-]*_bg\.wasm' | head -1 || true)
        verify_published "$expect"
    fi
    exit 0
fi

# The site is force-replaced wholesale, so deploying a stale checkout would
# roll back whatever else lives on main — most importantly the data mirror.
if git fetch -q origin main 2>/dev/null; then
    if ! git merge-base --is-ancestor origin/main HEAD; then
        behind=$(git rev-list --count HEAD..origin/main)
        if [ "$ALLOW_STALE" = 1 ]; then
            echo "!! deploying a stale checkout ($behind commit(s) behind origin/main)"
        else
            echo "refusing to deploy: origin/main is $behind commit(s) ahead of HEAD." >&2
            echo "The site is replaced wholesale, so this would roll back those" >&2
            echo "commits (e.g. the CMI data mirror). Run 'git pull' first, or" >&2
            echo "pass --allow-stale if that is what you want." >&2
            exit 1
        fi
    fi
else
    echo "!! could not reach origin — deploying from the local checkout as-is"
fi

# --sync: refresh the data mirror before building (what the cron used to do).
# The gate lives in the sync binary: a failure leaves the last good mirror in
# place, and we stop rather than ship a build around bad data.
if [ "$SYNC_DATA" = 1 ]; then
    echo "==> refreshing the CMI data mirror"
    command -v cargo >/dev/null || { echo "--sync needs cargo on PATH" >&2; exit 1; }
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE/target}" \
        cargo run --release -q -p cmi-timetable-sync -- app/public/data
    if [ -n "$(git status --porcelain -- app/public/data)" ]; then
        git add app/public/data
        git commit -q -m "data: refresh CMI mirror" -- app/public/data
        SHA=$(git rev-parse --short HEAD)
        [ -z "$(git status --porcelain)" ] && DIRTY=""
        echo "    mirror updated and committed ($SHA)"
    else
        echo "    mirror already current"
    fi
fi

# Build steps, identical in both environments. Paths are resolved at RUN
# time (escaped \$PWD) because the repo root is $PWD on the host but /work
# inside the container.
build_script() {
    cat <<'SCRIPT'
set -euo pipefail
CACHE="$PWD/.build-cache"
BIN="$CACHE/bin"
mkdir -p "$BIN"

# Reuse a trunk already on PATH; otherwise fetch the release binary for
# this OS/arch (cache key includes both, so a shared .build-cache between a
# container and the host can't hand over an unrunnable binary).
if command -v trunk >/dev/null; then
    TRUNK=$(command -v trunk)
else
    case "$(uname -m)" in
        x86_64|amd64)   arch=x86_64 ;;
        aarch64|arm64)  arch=aarch64 ;;
        *) echo "no trunk release for $(uname -m) — install trunk yourself" >&2; exit 1 ;;
    esac
    case "$(uname -s)" in
        Linux)  if ldd --version 2>&1 | grep -qi musl; then
                    target="$arch-unknown-linux-musl"
                else
                    target="$arch-unknown-linux-gnu"
                fi ;;
        Darwin) target="$arch-apple-darwin" ;;
        *) echo "unsupported OS $(uname -s) — install trunk yourself" >&2; exit 1 ;;
    esac
    TRUNK="$BIN/trunk-$TRUNK_VERSION-$target"
    if [ ! -x "$TRUNK" ]; then
        echo "==> downloading trunk $TRUNK_VERSION ($target)"
        tmp=$(mktemp -d "$CACHE/trunk-dl.XXXXXX")
        curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/$TRUNK_VERSION/trunk-$target.tar.gz" \
            | tar -xzf- -C "$tmp"
        mv "$tmp/trunk" "$TRUNK"
        rm -rf "$tmp"
    fi
fi

if [ "$SKIP_TESTS" != 1 ]; then
    echo "==> running the test suite"
    cargo test --workspace --quiet
fi
echo "==> building (public URL: $PUBLIC_URL)"
cd app && "$TRUNK" build --release --public-url "$PUBLIC_URL" --dist dist-deploy
SCRIPT
}

rm -rf "$DIST"
mkdir -p "$CACHE"

if [ "${FORCE_LOCAL:-0}" != 1 ] && docker info >/dev/null 2>&1; then
    echo "==> building in a temporary Docker container ($DOCKER_IMAGE)"
    # Rootless Docker maps the container root to this user, so its writes
    # already land as ours; chowning to our uid inside the namespace would
    # actually make them unreadable.
    ROOTLESS=0
    if docker info -f '{{println .SecurityOptions}}' 2>/dev/null | grep -q rootless; then
        ROOTLESS=1
    fi
    docker run --rm -v "$PWD":/work -w /work \
        -e CARGO_TARGET_DIR=/work/.build-cache/target \
        -e CARGO_HOME=/work/.build-cache/cargo \
        -e XDG_CACHE_HOME=/work/.build-cache/xdg \
        -e TRUNK_VERSION="$TRUNK_VERSION" \
        -e PUBLIC_URL="$PUBLIC_URL" \
        -e SKIP_TESTS="$SKIP_TESTS" \
        "$DOCKER_IMAGE" bash -c "
            set -euo pipefail
            # Hand the build's artifacts back to the host user even when the
            # build fails, so a red test run can't leave root-owned files.
            $([ "$ROOTLESS" = 1 ] || echo "trap 'chown -R $(id -u):$(id -g) /work/.build-cache /work/$DIST 2>/dev/null || true' EXIT")
            # build.rs shells out to git; without this the repo's host
            # ownership trips git's dubious-ownership check and the commit
            # stamp silently becomes \"unknown\".
            git config --global --add safe.directory /work
            rustup target add wasm32-unknown-unknown >/dev/null
            $(build_script)
        "
else
    echo "==> Docker unavailable — building locally"
    command -v cargo >/dev/null || { echo "need cargo on PATH (or Docker)" >&2; exit 1; }
    # Distro toolchains (Arch, Fedora, …) ship the wasm target and have no
    # rustup at all, so only ask rustup for it when rustup is actually here.
    if command -v rustup >/dev/null; then
        rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown \
            || rustup target add wasm32-unknown-unknown
    elif ! rustc --print target-libdir --target wasm32-unknown-unknown >/dev/null 2>&1; then
        echo "this toolchain has no wasm32-unknown-unknown target — install it" >&2
        echo "(Arch: pacman -S rust-wasm · rustup: rustup target add wasm32-unknown-unknown)" >&2
        exit 1
    fi
    # A dedicated target dir so a running `trunk serve` can't race this build.
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE/target}"
    export TRUNK_VERSION PUBLIC_URL SKIP_TESTS
    bash -c "$(build_script)"
fi

[ -f "$DIST/index.html" ] || { echo "build produced no $DIST/index.html" >&2; exit 1; }

if [ "$BUILD_ONLY" = 1 ]; then
    echo "==> build ok ($(dist_fingerprint "$DIST" || echo "?")) — publishing nothing (--build-only)"
    exit 0
fi

echo "==> publishing to the $BRANCH branch (single orphan commit)"
touch "$DIST/.nojekyll"   # skip Pages' Jekyll pass
# Build the commit in a scratch index inside the real repo: the working tree
# and the normal index stay untouched, and the push uses this repo's own
# remotes and credentials.
GIT_DIR_ABS=$(git rev-parse --absolute-git-dir)
IDX="$CACHE/pages.index"
rm -f "$IDX"
(
    cd "$DIST"
    GIT_DIR="$GIT_DIR_ABS" GIT_WORK_TREE="$PWD" GIT_INDEX_FILE="$IDX" \
        git add --all --force .
)
TREE=$(GIT_DIR="$GIT_DIR_ABS" GIT_INDEX_FILE="$IDX" git write-tree)
export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-$(git config user.name || echo deploy)}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-$(git config user.email || echo deploy@localhost)}"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
COMMIT=$(git commit-tree "$TREE" \
    -m "deploy: $SHA$DIRTY $(date -u +%Y-%m-%dT%H:%M:%SZ)")
rm -f "$IDX"
git push -q --force "$ORIGIN" "$COMMIT:refs/heads/$BRANCH"

echo "==> published $SHA$DIRTY to $BRANCH${SITE:+ — $SITE}"
if [ "$VERIFY" = 1 ]; then
    verify_published "$(dist_fingerprint "$DIST" || true)"
fi

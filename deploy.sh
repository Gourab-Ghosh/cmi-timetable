#!/usr/bin/env bash
# Deploy the app to GitHub Pages from THIS machine — no GitHub-hosted
# runners in the path, so a GitHub Actions outage can never block a release.
#
#   ./deploy.sh                  # test + build + publish
#   ./deploy.sh --skip-tests     # publish without running the test suite
#   ./deploy.sh --allow-stale    # publish even if origin/main is ahead
#
# How it works:
#   1. Builds the app with Trunk inside a temporary Docker container
#      (rust:1); if Docker is unavailable, builds locally instead.
#   2. Publishes the built site as a SINGLE orphan commit on the `gh-pages`
#      branch (built in a scratch index, so the working tree is untouched)
#      and force-pushes it. `main` never carries build artifacts, and the
#      branch keeps no history — repo visitors only ever see source.
#   3. GitHub Pages (set to serve the `gh-pages` branch) picks it up in a
#      minute or two.
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
for arg in "$@"; do
    case "$arg" in
        --skip-tests)  SKIP_TESTS=1 ;;
        --allow-stale) ALLOW_STALE=1 ;;
        -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
        *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
    esac
done

command -v git >/dev/null || { echo "git is required" >&2; exit 1; }
ORIGIN=$(git remote get-url origin)
REPO_NAME=$(basename -s .git "$ORIGIN")
PUBLIC_URL="${PUBLIC_URL:-/$REPO_NAME/}"
SHA=$(git rev-parse --short HEAD)
DIRTY=""
[ -z "$(git status --porcelain)" ] || DIRTY="+dirty"

# The site is force-replaced wholesale, so deploying a stale checkout would
# roll back whatever else lives on main — most importantly the data mirror
# the sync cron commits every six hours.
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
    rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown \
        || rustup target add wasm32-unknown-unknown
    # A dedicated target dir so a running `trunk serve` can't race this build.
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE/target}"
    export TRUNK_VERSION PUBLIC_URL SKIP_TESTS
    bash -c "$(build_script)"
fi

[ -f "$DIST/index.html" ] || { echo "build produced no $DIST/index.html" >&2; exit 1; }

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

if [[ "$ORIGIN" =~ github.com[:/]([^/]+)/([^/]+?)(\.git)?$ ]]; then
    owner=$(echo "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')
    echo "==> published $SHA$DIRTY — https://$owner.github.io$PUBLIC_URL"
    echo "    (Pages rebuilds within a minute or two)"
else
    echo "==> published $SHA$DIRTY to $BRANCH on $ORIGIN"
fi

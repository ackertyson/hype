#!/usr/bin/env bash
# Bump the version, commit, tag and push. Pushing the tag runs
# .github/workflows/release.yml, which publishes the prebuilt binaries the
# Homebrew tap (ackertyson/homebrew-hype) downloads.
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "Usage: $0 <version>" >&2
  echo "Example: $0 0.2.0" >&2
  exit 1
fi

VERSION="${1#v}"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must be semver (e.g. 1.2.3)" >&2
  exit 1
fi

cd "$(git rev-parse --show-toplevel)"

BRANCH=$(git branch --show-current)
if [ "$BRANCH" != "master" ]; then
  echo "Error: releases must be tagged from master (currently on $BRANCH)" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: working tree is dirty — commit or stash changes first" >&2
  exit 1
fi

git fetch origin "$BRANCH" --quiet
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse "origin/${BRANCH}")
if [ "$LOCAL" != "$REMOTE" ]; then
  echo "Error: local $BRANCH ($LOCAL) differs from origin/$BRANCH ($REMOTE)" >&2
  echo "Pull or push first." >&2
  exit 1
fi

if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
  echo "Error: tag v${VERSION} already exists" >&2
  exit 1
fi

CURRENT=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ "$CURRENT" = "$VERSION" ]; then
  echo "Error: Cargo.toml already has version ${VERSION}" >&2
  exit 1
fi

sed -i '' "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml
if ! grep -q "^version = \"${VERSION}\"" Cargo.toml; then
  echo "Error: failed to update version in Cargo.toml" >&2
  exit 1
fi

# Regenerate Cargo.lock with the new version.
cargo check --quiet

git add Cargo.toml Cargo.lock
git commit -m "Bump to v${VERSION}"
git tag "v${VERSION}"

echo "Ready to push v${VERSION}. Review:"
git log --oneline -1
echo ""
read -rp "Push to origin/${BRANCH} with tag? [y/N] " confirm
if [[ "$confirm" =~ ^[Yy]$ ]]; then
  git push origin "$BRANCH"
  git push origin "v${VERSION}"
  echo "Pushed v${VERSION}. Once the Release workflow finishes, run"
  echo "release.sh ${VERSION} in homebrew-hype."
else
  echo "Aborted. To undo: git reset --hard HEAD~1 && git tag -d v${VERSION}"
fi

#!/usr/bin/env bash
# changelog.sh — Generate a changelog entry for the current milestone
#
# Run by an AI agent to append release notes to CHANGELOG.md.
#
# Usage:
#   bash scripts/changelog.sh                      # Auto-detect last tag → milestone
#   bash scripts/changelog.sh --tag v0.1.0-alpha.4 # Use specific tag/milestone
#   bash scripts/changelog.sh --dry-run             # Print to stdout only
#
# How it works:
#   1. Find the last git tag (or use --tag override)
#   2. Query Gitea for closed issues/PRs with that milestone
#   3. Format as a changelog section
#   4. Prepend to CHANGELOG.md
#
# Prerequisites:
#   - GITEA_TOKEN env var (personal access token with repo read scope)
#   - GITEA_URL env var (default: http://tesla.local:3000)
#   - curl, jq installed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CHANGELOG_FILE="$PROJECT_ROOT/CHANGELOG.md"

# ── Config ────────────────────────────────────────────────────────────────────
GITEA_URL="${GITEA_URL:-http://tesla.local:3000}"
GITEA_TOKEN="${GITEA_TOKEN:-}"
REPO_OWNER="danielvela"
REPO_NAME="seneschal"
API_BASE="${GITEA_URL}/api/v1"

DRY_RUN=false
TARGET_TAG=""

# ── Parse args ────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      TARGET_TAG="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --help|-h)
      sed -n 's/^# \?//p' "$0" | head -16
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# ── Auth check ────────────────────────────────────────────────────────────────
if [[ -z "$GITEA_TOKEN" ]]; then
  echo "ERROR: GITEA_TOKEN not set." >&2
  echo "Export a personal access token: export GITEA_TOKEN=your_token" >&2
  exit 1
fi

# ── Determine target tag ─────────────────────────────────────────────────────
if [[ -z "$TARGET_TAG" ]]; then
  TARGET_TAG=$(git -C "$PROJECT_ROOT" tag --sort=-v:refname | head -1)
  if [[ -z "$TARGET_TAG" ]]; then
    echo "ERROR: No tags found in repository." >&2
    exit 1
  fi
fi

echo "Generating changelog for: ${TARGET_TAG}"

# ── Helper: authenticated curl ────────────────────────────────────────────────
gcurl() {
  curl -s -H "Authorization: token ${GITEA_TOKEN}" "$@"
}

# ── Step 1: Find milestone by title ──────────────────────────────────────────
MILESTONE_ID=$(gcurl "${API_BASE}/repos/${REPO_OWNER}/${REPO_NAME}/milestones?state=all&per_page=50" \
  | jq -r --arg title "$TARGET_TAG" '.[] | select(.title == $title) | .id' | head -1)

if [[ -z "$MILESTONE_ID" || "$MILESTONE_ID" == "null" ]]; then
  echo "WARNING: No milestone '${TARGET_TAG}' found. Falling back to git log." >&2

  PREV_TAG=$(git -C "$PROJECT_ROOT" tag --sort=-v:refname | grep -B1 "^${TARGET_TAG}$" | tail -1 || true)
  if [[ -n "$PREV_TAG" ]]; then
    RANGE="${PREV_TAG}..${TARGET_TAG}"
  else
    RANGE="${TARGET_TAG}"
  fi

  TAG_DATE=$(git -C "$PROJECT_ROOT" log -1 --format=%ci "$TARGET_TAG" 2>/dev/null | cut -d' ' -f1 || date +%Y-%m-%d)

  SECTION="## ${TARGET_TAG} (${TAG_DATE})

### Changes

"
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    MSG="${line#* }"  # strip SHA prefix
    if [[ "$MSG" =~ ^feat(:|\ ) ]]; then
      SECTION+="- ✨ ${MSG#*[a-z]}: "
      SECTION+=$(echo "${MSG}" | sed 's/^feat[^:]*: *//')
      SECTION+=$'\n'
    elif [[ "$MSG" =~ ^fix(:|\ ) ]]; then
      SECTION+="- 🐛 $(echo "${MSG}" | sed 's/^fix[^:]*: *//')"
      SECTION+=$'\n'
    elif [[ "$MSG" =~ ^refactor(:|\ ) ]]; then
      SECTION+="- 🔧 $(echo "${MSG}" | sed 's/^refactor[^:]*: *//')"
      SECTION+=$'\n'
    elif [[ "$MSG" =~ ^docs(:|\ ) ]]; then
      SECTION+="- 📝 $(echo "${MSG}" | sed 's/^docs[^:]*: *//')"
      SECTION+=$'\n'
    else
      SECTION+="- ${MSG}"
      SECTION+=$'\n'
    fi
  done < <(git -C "$PROJECT_ROOT" log --oneline --no-merges "$RANGE" 2>/dev/null)

  SECTION+=$'\n'

  # Output
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "$SECTION"
  elif [[ -f "$CHANGELOG_FILE" ]]; then
    TEMP_FILE=$(mktemp)
    head -1 "$CHANGELOG_FILE" > "$TEMP_FILE"
    echo "" >> "$TEMP_FILE"
    echo "$SECTION" >> "$TEMP_FILE"
    tail -n +2 "$CHANGELOG_FILE" >> "$TEMP_FILE"
    mv "$TEMP_FILE" "$CHANGELOG_FILE"
    echo "✅ Updated ${CHANGELOG_FILE}"
  else
    { echo "# Changelog"; echo ""; echo "All notable changes to this project."; echo ""; echo "$SECTION"; } > "$CHANGELOG_FILE"
    echo "✅ Created ${CHANGELOG_FILE}"
  fi
  exit 0
fi

echo "Found milestone #${MILESTONE_ID}"

# ── Step 2: Fetch issues ─────────────────────────────────────────────────────
ISSUES=$(gcurl "${API_BASE}/repos/${REPO_OWNER}/${REPO_NAME}/issues?milestone=${MILESTONE_ID}&state=closed&per_page=100")
PRS=$(gcurl "${API_BASE}/repos/${REPO_OWNER}/${REPO_NAME}/pulls?milestone=${MILESTONE_ID}&state=closed&per_page=100")

# ── Step 3: Get release date ─────────────────────────────────────────────────
TAG_DATE=$(git -C "$PROJECT_ROOT" log -1 --format=%ci "$TARGET_TAG" 2>/dev/null | cut -d' ' -f1 || date +%Y-%m-%d)

# ── Step 4: Build changelog section ──────────────────────────────────────────
SECTION="## ${TARGET_TAG} (${TAG_DATE})

"

# Issues
ISSUE_COUNT=$(echo "$ISSUES" | jq 'length')
if [[ "$ISSUE_COUNT" -gt 0 ]]; then
  # Separate by labels
  FEATURES=$(echo "$ISSUES" | jq -r '[.[] | select(.labels[]?.name == "enhancement")] | .[] | "- **#" + (.number|tostring) + "**: " + .title' 2>/dev/null || echo "")
  BUGFIXES=$(echo "$ISSUES" | jq -r '[.[] | select(.labels[]?.name == "bug")] | .[] | "- **#" + (.number|tostring) + "**: " + .title' 2>/dev/null || echo "")
  OTHER_ISSUES=$(echo "$ISSUES" | jq -r '[.[] | select(.labels | not or (map(.name) | index("enhancement") | not) and (map(.name) | index("bug") | not))] | .[] | "- **#" + (.number|tostring) + "**: " + .title' 2>/dev/null || echo "")

  if [[ -n "$FEATURES" ]]; then
    SECTION+="### Features
${FEATURES}
"
  fi
  if [[ -n "$BUGFIXES" ]]; then
    SECTION+="### Bug Fixes
${BUGFIXES}
"
  fi
  if [[ -n "$OTHER_ISSUES" ]]; then
    SECTION+="### Other
${OTHER_ISSUES}
"
  fi
fi

# PRs (only if not already counted as issues)
PR_COUNT=$(echo "$PRS" | jq 'length')
if [[ "$PR_COUNT" -gt 0 ]]; then
  SECTION+="### Pull Requests
$(echo "$PRS" | jq -r '.[] | "- **#" + (.number|tostring) + "**: " + .title')
"
fi

SECTION+=$'\n'

# ── Step 5: Write to CHANGELOG.md ────────────────────────────────────────────
if [[ "$DRY_RUN" == "true" ]]; then
  echo "---"
  echo "$SECTION"
  echo "---"
  echo "(Dry run — no files modified)"
elif [[ -f "$CHANGELOG_FILE" ]]; then
  TEMP_FILE=$(mktemp)
  head -1 "$CHANGELOG_FILE" > "$TEMP_FILE"
  echo "" >> "$TEMP_FILE"
  echo "$SECTION" >> "$TEMP_FILE"
  tail -n +2 "$CHANGELOG_FILE" >> "$TEMP_FILE"
  mv "$TEMP_FILE" "$CHANGELOG_FILE"
  echo "✅ Updated ${CHANGELOG_FILE}"
else
  { echo "# Changelog"; echo ""; echo "All notable changes to this project."; echo ""; echo "$SECTION"; } > "$CHANGELOG_FILE"
  echo "✅ Created ${CHANGELOG_FILE}"
fi
#!/usr/bin/env bash
# Creates a git worktree for an isolated Claude Code session.
# Usage: bash scripts/claude-worktree.sh <task-name>
# Example: bash scripts/claude-worktree.sh fix-bt-codec
#
# This creates:
#   ../HKInvoke-<task-name>/  — independent working directory
#   branch work/<task-name>   — branched from current HEAD
#
# After the session, merge and clean up:
#   cd G:\HKInvoke
#   git merge work/<task-name>
#   git worktree remove ../HKInvoke-<task-name>

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASE_BRANCH="$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <task-name>"
    echo ""
    echo "Creates a worktree at ../HKInvoke-<task-name> branched from $BASE_BRANCH"
    echo ""
    echo "Examples:"
    echo "  $0 fix-bt-codec"
    echo "  $0 web-dashboard"
    echo "  $0 rootfs-cleanup"
    echo ""
    echo "Active worktrees:"
    git -C "$REPO_ROOT" worktree list
    exit 1
fi

TASK="$1"
BRANCH="work/$TASK"
WORKTREE_DIR="$(dirname "$REPO_ROOT")/HKInvoke-$TASK"

# Check if worktree already exists
if [ -d "$WORKTREE_DIR" ]; then
    echo "Worktree already exists: $WORKTREE_DIR"
    echo "Launching Claude Code in existing worktree..."
    cd "$WORKTREE_DIR"
    exec claude
fi

# Check if branch already exists
if git -C "$REPO_ROOT" show-ref --verify --quiet "refs/heads/$BRANCH" 2>/dev/null; then
    echo "Branch $BRANCH already exists. Reusing it."
    git -C "$REPO_ROOT" worktree add "$WORKTREE_DIR" "$BRANCH"
else
    git -C "$REPO_ROOT" worktree add -b "$BRANCH" "$WORKTREE_DIR" "$BASE_BRANCH"
fi

echo ""
echo "Worktree created:"
echo "  Directory: $WORKTREE_DIR"
echo "  Branch:    $BRANCH"
echo "  Base:      $BASE_BRANCH"
echo ""
echo "Launching Claude Code..."

cd "$WORKTREE_DIR"
exec claude

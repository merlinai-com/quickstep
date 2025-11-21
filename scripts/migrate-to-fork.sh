#!/bin/bash
# Migration script to convert clone to fork
# Usage: ./migrate-to-fork.sh YOUR_GITHUB_USERNAME

set -e

if [ -z "$1" ]; then
    echo "Usage: $0 YOUR_GITHUB_USERNAME"
    echo "Example: $0 JulianDarley"
    exit 1
fi

GITHUB_USER="$1"
FORK_URL="https://github.com/${GITHUB_USER}/quickstep.git"

echo "🚀 Migrating Quickstep to your fork..."
echo ""

# Step 1: Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Not in quickstep directory"
    exit 1
fi

# Step 2: Check current remote
echo "📋 Current remote:"
git remote -v
echo ""

# Step 3: Check if fork exists
echo "🔍 Checking if fork exists at ${FORK_URL}..."
if git ls-remote "${FORK_URL}" &>/dev/null; then
    echo "✅ Fork found!"
else
    echo "❌ Fork not found at ${FORK_URL}"
    echo ""
    echo "Please fork the repo first:"
    echo "1. Go to: https://github.com/RaphaelDarley/quickstep"
    echo "2. Click the 'Fork' button"
    echo "3. Then run this script again"
    exit 1
fi

# Step 4: Save current changes
echo ""
echo "💾 Staging local changes..."
git add .gitignore design/

# Step 5: Commit local changes if any
if ! git diff --cached --quiet; then
    echo "📝 Committing local changes..."
    git commit -m "Add design docs and update .gitignore"
else
    echo "ℹ️  No changes to commit"
fi

# Step 6: Change remote to fork
echo ""
echo "🔄 Changing remote to your fork..."
git remote set-url origin "${FORK_URL}"
git remote add upstream https://github.com/RaphaelDarley/quickstep.git 2>/dev/null || echo "ℹ️  Upstream already exists"

# Step 7: Verify
echo ""
echo "✅ Migration complete!"
echo ""
echo "📋 New remotes:"
git remote -v
echo ""
echo "📤 To push your changes:"
echo "   git push origin main"
echo ""
echo "📥 To pull updates from Raphael's repo:"
echo "   git fetch upstream"
echo "   git merge upstream/main"


#!/bin/bash
# Migration script to move to merlinai-com organization repo
# Usage: ./migrate-to-org-repo.sh

set -e

REPO_URL="https://github.com/merlinai-com/quickstep.git"

echo "🚀 Migrating Quickstep to merlinai-com/quickstep..."
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

# Step 3: Check if new repo exists
echo "🔍 Checking if repo exists at ${REPO_URL}..."
if git ls-remote "${REPO_URL}" &>/dev/null; then
    echo "✅ Repo found!"
else
    echo "❌ Repo not found at ${REPO_URL}"
    echo ""
    echo "Please create the repo first:"
    echo "1. Go to: https://github.com/orgs/merlinai-com/repositories"
    echo "2. Click 'New repository'"
    echo "3. Name it: quickstep"
    echo "4. Make it Public"
    echo "5. DO NOT initialize with README, .gitignore, or license (we have those)"
    echo "6. Then run this script again"
    exit 1
fi

# Step 4: Stage and commit local changes
echo ""
echo "💾 Staging local changes..."
git add .gitignore design/

# Step 5: Commit local changes if any
if ! git diff --cached --quiet; then
    echo "📝 Committing local changes..."
    git commit -m "Add design docs and update .gitignore

- Added design documentation and analysis
- Added bf-tree-docs reference materials
- Updated .gitignore to exclude bf-tree-docs submodule"
else
    echo "ℹ️  No changes to commit"
fi

# Step 6: Change remote to new org repo
echo ""
echo "🔄 Changing remote to merlinai-com/quickstep..."
git remote set-url origin "${REPO_URL}"

# Step 7: Add upstream for reference (optional)
echo ""
read -p "Add Raphael's repo as 'upstream' remote for reference? (y/n) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    git remote add upstream https://github.com/RaphaelDarley/quickstep.git 2>/dev/null || echo "ℹ️  Upstream already exists"
fi

# Step 8: Verify
echo ""
echo "✅ Migration complete!"
echo ""
echo "📋 New remotes:"
git remote -v
echo ""
echo "📤 To push to your new repo:"
echo "   git push -u origin main"
echo ""
if git remote | grep -q upstream; then
    echo "📥 To pull updates from Raphael's repo (if needed):"
    echo "   git fetch upstream"
    echo "   git merge upstream/main"
fi


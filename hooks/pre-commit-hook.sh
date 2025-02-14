#!/bin/bash

# Get the current branch name
BRANCH_NAME=$(git symbolic-ref --short HEAD)

# Define protected branches
PROTECTED_BRANCHES=("main" "develop")

# Check if the current branch is protected
for BRANCH in "${PROTECTED_BRANCHES[@]}"; do
  if [[ "$BRANCH_NAME" == "$BRANCH" ]]; then
    echo "❌ Direct commits to '$BRANCH_NAME' are not allowed!"
    exit 1
  fi
done

# Run Clippy (Linting)
echo "🔍 Running cargo clippy..."
cargo clippy --workspace --all-targets --all-features
CLIPPY_EXIT_CODE=$?
if [ $CLIPPY_EXIT_CODE -ne 0 ]; then
  echo "❌ cargo clippy failed!"
  exit $CLIPPY_EXIT_CODE
fi

# Run Rustfmt (Formatting Check)
echo "📝 Checking code format with cargo fmt..."
cargo +nightly fmt --all -- --check
FMT_EXIT_CODE=$?
if [ $FMT_EXIT_CODE -ne 0 ]; then
  echo "❌ Code formatting check failed!"
  exit $FMT_EXIT_CODE
fi

echo "✅ All checks passed!"
exit 0

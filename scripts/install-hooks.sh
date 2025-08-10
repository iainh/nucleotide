#!/bin/bash
# Script to install git hooks for the project

echo "Installing git hooks..."

# Create hooks directory if it doesn't exist
mkdir -p .git/hooks

# Install pre-commit hook
cat > .git/hooks/pre-commit << 'EOF'
#!/bin/sh
# Pre-commit hook to run cargo fmt before committing

# Check if we're in a Rust project
if [ -f "Cargo.toml" ]; then
    echo "Running cargo fmt..."
    
    # Run cargo fmt on all Rust files
    cargo fmt --all -- --check
    
    # If formatting check fails, format the files and inform the user
    if [ $? -ne 0 ]; then
        echo "Formatting issues detected. Running cargo fmt to fix..."
        cargo fmt --all
        
        echo ""
        echo "Files have been formatted. Please review the changes and commit again."
        echo "You can see what changed with: git diff"
        exit 1
    fi
    
    echo "Formatting check passed!"
fi

# Allow the commit to proceed
exit 0
EOF

# Make the hook executable
chmod +x .git/hooks/pre-commit

echo "✓ Pre-commit hook installed successfully!"
echo ""
echo "The pre-commit hook will:"
echo "  - Run 'cargo fmt --check' before each commit"
echo "  - Auto-format code if needed and ask you to review changes"
echo "  - Ensure consistent code formatting across the project"
#!/usr/bin/env bash
set -euo pipefail

echo "Checking layered architecture dependencies..."

# Function to check if a dependency exists
check_dep() {
    local from_crate=$1
    local to_crate=$2
    
    # Use --edges normal to only show dependencies, not reverse dependencies
    # Use --depth 10 to check transitive dependencies
    if cargo tree -p "$from_crate" --edges normal --depth 10 2>/dev/null | grep -q "^.*$to_crate v"; then
        return 0
    else
        return 1
    fi
}

# Check for forbidden dependencies
ERRORS=0

# nucleotide-editor should not depend on nucleotide-ui
if check_dep "nucleotide-editor" "nucleotide-ui"; then
    echo "❌ ERROR: nucleotide-editor depends on nucleotide-ui (forbidden)"
    ERRORS=$((ERRORS + 1))
else
    echo "✅ nucleotide-editor does not depend on nucleotide-ui"
fi

# nucleotide-types should not have heavy deps (when compiled without features)
echo "Checking nucleotide-types for heavy dependencies..."
if cargo tree -p nucleotide-types --no-default-features --edges normal 2>/dev/null | grep -E "^.*[(]?gpui|helix-core|helix-view|helix-term" | grep -v "^nucleotide-types"; then
    echo "❌ ERROR: nucleotide-types has heavy dependencies without feature flags"
    ERRORS=$((ERRORS + 1))
else
    echo "✅ nucleotide-types has no heavy dependencies (without features)"
fi

# Check that lower layers don't depend on higher layers
# Layer order: types < events < core < editor/workspace/lsp/ui < nucleotide
LAYERS=(
    "nucleotide-types"
    "nucleotide-events" 
    "nucleotide-core"
    "nucleotide-editor nucleotide-workspace nucleotide-lsp nucleotide-ui"
    "nucleotide"
)

echo "Checking layer dependencies..."
for i in "${!LAYERS[@]}"; do
    for lower_crate in ${LAYERS[$i]}; do
        for j in $(seq $((i + 1)) $((${#LAYERS[@]} - 1))); do
            for higher_crate in ${LAYERS[$j]}; do
                if [[ "$lower_crate" != "$higher_crate" ]]; then
                    if check_dep "$lower_crate" "$higher_crate"; then
                        echo "❌ ERROR: $lower_crate (layer $i) depends on $higher_crate (layer $j)"
                        ERRORS=$((ERRORS + 1))
                    fi
                fi
            done
        done
    done
done

if [ $ERRORS -eq 0 ]; then
    echo "✅ All layering checks passed!"
    exit 0
else
    echo "❌ Found $ERRORS layering violations"
    exit 1
fi
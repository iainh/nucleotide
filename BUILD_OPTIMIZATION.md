# Build Optimization Guide

This project has been configured with several build optimizations to improve compilation times.

## Build Modes

### Incremental Compilation (Default)
**Best for:** Iterative development, small code changes
```bash
cargo build              # Dev build
cargo build --release    # Release build
```

**Advantages:**
- Fastest rebuilds when making small changes
- Only recompiles changed modules
- Ideal for the edit-compile-test cycle

**Disadvantages:**
- Slower initial builds
- Can accumulate cruft over time
- Not compatible with sccache

### Sccache (Compilation Cache)
**Best for:** Clean builds, switching branches, CI
```bash
build-cached              # Dev build with sccache
build-release-cached      # Release build with sccache
```

**Advantages:**
- Caches compilation artifacts across builds
- Excellent for clean builds after cache warm-up
- Great when switching between branches
- Shared cache across all projects

**Disadvantages:**
- Disables incremental compilation
- First build is slower (cache population)
- Requires more disk space for cache

## Optimizations Applied

### 1. Cargo Profile Optimizations
- **Dev builds**: `debug = "line-tables-only"` - Minimal debug info for faster builds
- **Release builds**: `lto = "thin"` - Faster linking than full LTO with minimal size increase
- **macOS**: `split-debuginfo` enabled for faster linking

### 2. Platform-Specific Linkers
- **macOS**: Native linker with dead code stripping
- **Linux**: mold linker (when available) - 5-10x faster than GNU ld

### 3. Build Tools
- **sccache**: Available for caching compilation artifacts
- **Incremental compilation**: Enabled by default for iterative development

## Recommendations

1. **Daily development**: Use default incremental compilation
   ```bash
   cargo build
   ```

2. **After git pull or branch switch**: Use sccache
   ```bash
   build-cached
   ```

3. **Release builds**: Use sccache for clean builds
   ```bash
   build-release-cached
   ```

4. **If builds feel slow**: Clean and rebuild with sccache
   ```bash
   cargo clean
   build-cached
   ```

## Benchmarks (Approximate)

On Apple M3 Pro:
- **Clean release build (full LTO)**: ~3-4 minutes
- **Clean release build (thin LTO)**: ~2-3 minutes (25-40% faster)
- **Incremental release rebuild**: ~30-60 seconds
- **Clean dev build**: ~1-2 minutes
- **Incremental dev rebuild**: ~5-15 seconds

## Troubleshooting

If you see the error:
```
sccache: increment compilation is prohibited
```
This is expected when `CARGO_INCREMENTAL=1` is set. Use the provided aliases which handle this automatically.

To check sccache statistics:
```bash
sccache --show-stats
```

To clear sccache cache:
```bash
sccache --stop-server
rm -rf ~/.cache/sccache
```
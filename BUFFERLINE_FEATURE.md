# Tab Bar (Bufferline) Feature

## Overview
The tab bar displays all open buffers as tabs at the top of the editor area. This feature respects Helix's bufferline configuration and can be toggled dynamically.

## Configuration
The tab bar visibility is controlled by the `bufferline` setting in Helix's configuration:

### Using Commands
You can change the setting at runtime using the `:set` command:

```
:set bufferline never    # Never show the tab bar
:set bufferline always   # Always show the tab bar  
:set bufferline multiple # Show only when multiple buffers are open (default)
```

### Using Configuration File
Add to `~/.config/helix/config.toml`:

```toml
[editor]
bufferline = "always"  # or "never" or "multiple"
```

## Features
- **Dynamic Visibility**: Changes take effect immediately when using `:set bufferline`
- **Tab Switching**: Click on tabs to switch between buffers
- **Close Buttons**: Each tab has a close button (×) to close the buffer
- **Modified Indicator**: A bullet (•) appears for unsaved buffers
- **Alphabetical Ordering**: Tabs are sorted alphabetically by file path for consistent ordering

## Implementation Details
The fix ensures that configuration changes trigger UI updates:
1. When `:set bufferline` is executed, Helix emits a `ConfigEvent`
2. The Nucleotide workspace handles `ConfigEvent` and calls `cx.notify()`
3. This triggers a re-render of the UI, updating the tab bar visibility

## Testing
Run `./test_bufferline_toggle.sh` to test the dynamic toggle functionality.
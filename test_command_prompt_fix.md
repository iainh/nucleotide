# Command Prompt Fix Test

## Issue Fixed:
1. Command prompt now only opens with `:` key
2. Command prompt always starts empty (no pre-filled text)

## Test Steps:

1. Run `cargo run` or `./target/debug/hxg`
2. Press various keys that might create prompts:
   - `/` for search
   - `?` for reverse search  
   - `f` for file picker
   - Other prompt-triggering keys
3. Verify that NO prompt appears in the native UI for these keys
4. Press `:` for command mode
5. Verify that ONLY the command prompt appears

## Expected Behavior:
- Only the `:` key should show a native command prompt
- Other prompts (search, rename, etc.) should not appear as native prompts
- They will continue to work in the terminal rendering mode

## Fix Applied:
The code now:
1. Tracks when the `:` key is pressed during input handling
2. Only calls `emit_overlays` (which includes prompts) for command mode
3. For other keys, calls `emit_overlays_except_prompt` which skips prompt creation
4. This ensures only command prompts (triggered by `:`) appear as native prompts
5. Other prompt types (search `/`, reverse search `?`, etc.) continue to work in terminal mode
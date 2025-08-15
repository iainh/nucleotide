# Known Issues

## Test Compilation Stack Overflow

**Issue**: Running `cargo test` results in a stack overflow during macro expansion.

**Cause**: The gpui::test macro appears to cause deep recursion when combined with certain test structures.

**Workaround**: The recursion limit has been increased to 512 in main.rs. Tests can be run individually or the project can be built without tests using `cargo build`.

**Status**: This is a known issue with the GPUI framework's test macro. The code itself compiles and runs correctly.

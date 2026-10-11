# Attribution

Nucleotide is built on the shoulders of giants. This file acknowledges the projects and people who made this possible.

## Core Dependencies

### Helix Editor
- **Project**: https://github.com/helix-editor/helix
- **License**: MPL-2.0
- **Role**: Provides the entire editing engine, including syntax highlighting, LSP support, and modal editing capabilities
- **Maintainers**: Blaž Hrastnik (@archseer) and the Helix community

The Cargo dependencies, vendored snapshots and Nix runtime use upstream revision
[`ba40e547`](https://github.com/helix-editor/helix/commit/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc).
The four vendored forks retain Nucleotide's integration patches:

- `helix-lsp`: external transports, remote workspace startup and completion capabilities.
- `helix-lsp-types`: completion `itemDefaults` deserialization.
- `helix-stdx`: SSH and WSL virtual paths.
- `helix-view`: backend opening, hydration, saving, opaque file versions, LSP URLs and pane splits.

The local document writer retains upstream's symlink-aware backup and unsupported
filesystem-sync handling. Writers supplied through `Document::save_with` remain
responsible for backend persistence and conflict detection. The default and base16
theme support files in `vendor/` are required by `helix-view` and must stay in place.

To check the forks, run
`cargo test --locked -p helix-view -p helix-stdx -p helix-lsp -p helix-lsp-types`.
Encoding fixtures in `vendor/helix-view/tests/encoding` come from the same upstream
revision, with their WHATWG licence. Missing fixtures fail tests rather than
silently skipping validation.

### GPUI Framework
- **Project**: https://github.com/zed-industries/zed
- **License**: Apache-2.0/MIT
- **Role**: Provides the GPU-accelerated UI framework for rendering
- **Maintainers**: Zed Industries

### Original helix-gpui
- **Project**: https://github.com/polachok/helix-gpui
- **Author**: Alexander Polakov (@polachok)
- **License**: MPL-2.0
- **Role**: The original GUI implementation that Nucleotide was forked from

### Phosphor Icons
- **Project/Core**: https://github.com/phosphor-icons/core
- **Website**: https://phosphoricons.com
- **License**: MIT
- **Role**: Provides the regular and duotone icons used throughout the interface
- **Maintainers**: Phosphor Icons team

## Special Thanks

- The Helix community for creating an amazing modal editor
- Zed Industries for open-sourcing GPUI
- Alexander Polakov for starting the helix-gpui project

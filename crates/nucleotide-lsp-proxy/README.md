# nucleotide-lsp-proxy

`nucleotide-lsp-proxy` is a diagnostic command-line helper for tracing Language Server Protocol (LSP) traffic. It is not part of the normal editor startup path. Nucleotide only uses it when LSP proxying is enabled explicitly.

The proxy starts the real language server as a child process, forwards stdio LSP messages between the editor and that server, and writes each JSON-RPC message body to a JSONL log file.

## How Nucleotide uses it

The main application does not link to this crate as a Rust dependency. The crate is a workspace binary so it can be built with the rest of the workspace.

When `NUCLEOTIDE_LSP_USE_PROXY=1` is set, `nucleotide-lsp` creates a temporary `PATH` shim with the same name as the language server. For example, if Helix is about to start `rust-analyzer`, Nucleotide writes a temporary executable named `rust-analyzer`. That shim executes `nucleotide-lsp-proxy`, passing the resolved real server path with `--server-cmd`.

Helix then starts the server through its normal registry path. Because the shim directory is first in `PATH`, Helix launches the proxy instead of the server directly.

## Enable proxy logging from the app

Build the proxy and make sure it is on `PATH`:

```sh
cargo build -p nucleotide-lsp-proxy
export PATH="$PWD/target/debug:$PATH"
```

Then start Nucleotide with proxying enabled:

```sh
NUCLEOTIDE_LSP_USE_PROXY=1 cargo run -p nucleotide
```

Proxy logs are written under `logs/lsp/` with names like:

```text
proxy-rust-analyzer-1730000000000.jsonl
```

Each line is a JSON object with:

- `ts`: UTC timestamp
- `direction`: `out` for editor-to-server messages or `in` for server-to-editor messages
- `method`: JSON-RPC method when present
- `id`: JSON-RPC request ID when present
- `raw`: The raw JSON message body

## Run it directly

You can also run the proxy as a standalone command:

```sh
nucleotide-lsp-proxy --server-cmd rust-analyzer --log logs/lsp/rust-analyzer.jsonl
```

Arguments after `--` are forwarded to the real server:

```sh
nucleotide-lsp-proxy --server-cmd rust-analyzer --log logs/lsp/rust-analyzer.jsonl -- --stdio
```

The proxy expects LSP stdio framing with `Content-Length` headers. It exits when either side of the stream closes, then attempts to terminate the child language server.

## Packaging note

Release packages currently copy the `nucl` application binary, not `nucleotide-lsp-proxy`. For packaged builds, this debug path only works if `nucleotide-lsp-proxy` is installed separately and discoverable on `PATH`.

## Privacy note

LSP messages can contain file paths, source snippets, symbols, diagnostics and project configuration. Treat proxy logs as sensitive project data.

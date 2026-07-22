# Eclipse JDT LS integration architecture

This document records the JDT LS protocol surface and the Nucleotide architecture for consuming it. It was audited in July 2026 against [`eclipse-jdtls/eclipse.jdt.ls` commit `4ce45732`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/commit/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83). The authoritative JDT LS entry points are:

- [`JDTLanguageServer`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/handlers/JDTLanguageServer.java): standard and custom request dispatch.
- [`InitHandler`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/handlers/InitHandler.java): advertised server capabilities.
- [`JavaProtocolExtensions`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/lsp/JavaProtocolExtensions.java): custom `java/*` methods.
- [`JavaClientConnection`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/JavaClientConnection.java): server-to-client extensions.
- [`plugin.xml`](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/4ce45732e9c1816b5807ae8edf6c1c2b70b5cf83/org.eclipse.jdt.ls.core/plugin.xml): built-in `workspace/executeCommand` registrations.

The old version of this document listed many plausible-looking commands that do not exist in JDT LS. The inventories below intentionally distinguish protocol methods, server commands, client commands, and optional bundle commands.

## Design goals and invariants

1. Standard LSP remains language-agnostic. Java uses the same completion, navigation, edit, diagnostics, hierarchy, and progress paths as every other server.
2. Non-standard transport is generic. Helix exposes raw JSON-RPC request and notification operations; it does not know about Java.
3. JDT LS wire names live in one adapter. Application and UI code must not scatter `java/*`, `language/*`, or Java command strings.
4. Stable contracts are strongly typed. Version-sensitive, additive refactoring payloads may cross the adapter as `serde_json::Value` until fixtures establish a stable local model.
5. Capabilities are truthful. An extended client capability stays `false` until transport, result handling, UI, cancellation, and tests all work end-to-end.
6. Commands are feature-detected from `executeCommandProvider.commands`; bundle commands must never be inferred from a JDT LS version.
7. Unknown event kinds and additive JSON fields are tolerated. Unknown commands are not executed.
8. JDT LS, java-debug, and java-test are separate, version-pinned components.

## Nucleotide layers

```diagram
┌────────────────────────────────────────────────────────────────────┐
│ GPUI/application                                                   │
│ actions, pickers, panels, virtual documents, edit confirmation     │
└──────────────────────────────┬─────────────────────────────────────┘
                               │ typed intent/result
┌──────────────────────────────▼─────────────────────────────────────┐
│ nucleotide-lsp                                                    │
│ generic extension traits + JDT LS protocol adapter + event decode │
└──────────────────────────────┬─────────────────────────────────────┘
                               │ typed marker or raw JSON fallback
┌──────────────────────────────▼─────────────────────────────────────┐
│ helix-lsp Client                                                  │
│ standard request<T> + request_value + notify_value + timeouts     │
└──────────────────────────────┬─────────────────────────────────────┘
                               │ JSON-RPC 2.0 over stdio
┌──────────────────────────────▼─────────────────────────────────────┐
│ JDT LS core                  │ optional trusted OSGi bundles       │
│ Java model, Maven, Gradle    │ java-debug / java-test commands     │
└──────────────────────────────┴─────────────────────────────────────┘
```

Implemented source boundaries:

- `vendor/helix-lsp/src/client.rs`: runtime-selected request/notification transport.
- `vendor/helix-lsp/src/lib.rs`: initialize-only option overlays, kept separate from `workspace/configuration` settings.
- `crates/nucleotide-lsp/src/extensions.rs`: generic typed request, notification, command, and inbound event contracts.
- `crates/nucleotide-lsp/src/jdtls/protocol.rs`: JDT method, command, model, and code-action-kind catalog.
- `crates/nucleotide-lsp/src/jdtls/mod.rs`: server matching, conservative capabilities, and custom notification decoding.
- `crates/nucleotide-lsp/src/helix_lsp_bridge.rs`: typed bridge and advertised-command check.
- `crates/nucleotide/src/application/mod.rs`: presentation of normalized status, actionable messages, events, and progress.

## Standard LSP surface implemented by JDT LS

JDT LS statically advertises a feature when the client says dynamic registration is unavailable. It dynamically registers many features when the client truthfully advertises dynamic registration. Nucleotide currently advertises dynamic registration only for watched files, which lets JDT LS return the other capabilities statically and avoids unsupported registration traffic.

### Lifecycle, synchronization, and diagnostics

| Direction | Methods | Notes |
| --- | --- | --- |
| Client → server | `initialize`, `initialized`, `shutdown`, `exit` | Initialization imports the Eclipse workspace asynchronously. `Started` and `ServiceReady` are distinct milestones. |
| Client → server | `$/setTrace`, `$/cancelRequest` | Trace is a JDT LS no-op; asynchronous operations honor cancellation. |
| Client → server | `textDocument/didOpen`, `didChange`, `didClose`, `didSave` | Incremental sync; saves include text. |
| Client → server | `textDocument/willSaveWaitUntil` | Java save actions/cleanups; advertised only when supported by the client. |
| Server → client | `textDocument/publishDiagnostics` | Push diagnostics only. JDT LS does not implement pull diagnostics. |

JDT LS may advertise `willSave`, but its implementation is effectively a no-op. It has no notebook service.

### Language intelligence and navigation

| Capability | Requests |
| --- | --- |
| Completion | `textDocument/completion`, `completionItem/resolve` |
| Hover/signatures | `textDocument/hover`, `textDocument/signatureHelp` |
| Navigation | `textDocument/definition`, `declaration`, `typeDefinition`, `implementation`, `references` |
| Document structure | `textDocument/documentHighlight`, `documentSymbol`, `foldingRange`, `selectionRange` |
| Workspace symbols | `workspace/symbol` |
| Inlay hints | `textDocument/inlayHint` (no resolve) |
| Semantic tokens | `textDocument/semanticTokens/full` only; no range or delta |

### Editing, refactoring, and source actions

| Capability | Requests |
| --- | --- |
| Formatting | `textDocument/formatting`, `rangeFormatting`, `onTypeFormatting` |
| Rename | `textDocument/prepareRename`, `rename` |
| File refactor | `workspace/willRenameFiles` for Java files/folders |
| Code actions | `textDocument/codeAction`, `codeAction/resolve` |
| Code lenses | `textDocument/codeLens`, `codeLens/resolve` |

On-type formatting triggers on `;`, newline, and `}`. Workspace edits may contain text edits plus create/rename/delete resource operations and must be applied atomically according to the negotiated failure mode.

### Hierarchies

| Capability | Requests |
| --- | --- |
| Call hierarchy | `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls` |
| Type hierarchy | `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes` |

The legacy `java.navigate.openTypeHierarchy` command remains for compatibility, but new UI should use the standard type-hierarchy protocol.

### Workspace and server-to-client requests

| Direction | Methods |
| --- | --- |
| Client → server | `workspace/didChangeConfiguration`, `didChangeWatchedFiles`, `didChangeWorkspaceFolders`, `executeCommand` |
| Server → client | `workspace/applyEdit`, `workspace/configuration`, `client/registerCapability`, `client/unregisterCapability` |
| Server → client | `window/logMessage`, `showMessage`, `showMessageRequest`, `window/workDoneProgress/create`, `$/progress` |
| Server → client | `workspace/inlayHint/refresh`, `workspace/codeLens/refresh`, `telemetry/event` when negotiated/configured |

Nucleotide resolves dotted `workspace/configuration` sections (for example `java.format.enabled`) rather than returning the entire configuration object for every item.

## Custom top-level methods: client to JDT LS

There are 29 core top-level extensions: 26 requests and 3 notifications. The marker types are centralized in `jdtls/protocol.rs`.

### Project, content, and search

| Method | Kind | Params → result |
| --- | --- | --- |
| `java/classFileContents` | request | `TextDocumentIdentifier → string` |
| `java/projectConfigurationUpdate` | notification, deprecated | `TextDocumentIdentifier` |
| `java/projectConfigurationsUpdate` | notification | `{ identifiers[] }` |
| `java/buildWorkspace` | request | `boolean → 0 FAILED | 1 SUCCEED | 2 WITH_ERROR | 3 CANCELLED` |
| `java/buildProjects` | request | project identifiers/full-build flag → build status |
| `java/cleanup` | request | `TextDocumentIdentifier → WorkspaceEdit` |
| `java/searchSymbols` | request | query/project/source/max-results filter → `SymbolInformation[]` |
| `java/findLinks` | request | document position/link type → locations with display metadata |
| `java/extendedDocumentSymbol` | request | `DocumentSymbolParams → ExtendedDocumentSymbol[]` |
| `java/validateDocument` | notification | `{ textDocument }` |

### Source generation

| Inspect request | Apply request | Purpose |
| --- | --- | --- |
| `java/listOverridableMethods` | `java/addOverridableMethods` | Override/implement methods |
| `java/checkHashCodeEqualsStatus` | `java/generateHashCodeEquals` | Generate `equals`/`hashCode` |
| `java/checkToStringStatus` | `java/generateToString` | Generate `toString` |
| `java/resolveUnimplementedAccessors` | `java/generateAccessors` | Generate getters/setters |
| `java/checkConstructorsStatus` | `java/generateConstructors` | Generate constructors |
| `java/checkDelegateMethodsStatus` | `java/generateDelegateMethods` | Generate delegate methods |
| — | `java/organizeImports` | Resolve and organize imports |

These are multi-step protocols. The client first obtains candidates, presents selection/configuration UI, then sends selected opaque IDs back to JDT LS. Nucleotide must not reconstruct Java bindings locally.

### Advanced refactoring

| Method | Purpose |
| --- | --- |
| `java/getRefactorEdit` | Compute extract/assign/move/change-signature/introduce-parameter/interface edits and status. |
| `java/getChangeSignatureInfo` | Obtain method, parameter, return, modifier, and exception metadata. |
| `java/inferSelection` | Infer valid extract-method/variable/field source ranges. |
| `java/getMoveDestinations` | Obtain valid package/type/resource move targets. |
| `java/move` | Compute the selected move refactor. |
| `java/checkExtractInterfaceStatus` | Obtain extract-interface candidates; edit comes from `getRefactorEdit`. |

Wire-visible refactor discriminators include `extractVariableAllOccurrence`, `extractVariable`, `assignVariable`, `assignField`, `extractMethod`, `extractConstant`, `convertVariableToField`, `extractField`, `moveFile`, `moveInstanceMethod`, `moveStaticMember`, `moveType`, `invertVariable`, `convertAnonymousClassToNestedCommand`, `introduceParameter`, `extractInterface`, and `changeSignature`.

## Core `workspace/executeCommand` IDs

JDT LS commands are open-ended: trusted OSGi bundles can contribute more IDs. Core currently registers these commands:

### Editing and content

- `java.edit.organizeImports`
- `java.edit.stringFormatting`
- `java.edit.handlePasteEvent`
- `java.edit.smartSemicolonDetection`
- `java.completion.onDidSelect`
- `java.decompile`
- `java.getFullyQualifiedName`
- `java.project.resolveText`

### Projects, source, and classpaths

- `java.project.resolveSourceAttachment`
- `java.project.updateSourceAttachment`
- `java.project.addToSourcePath`
- `java.project.removeFromSourcePath`
- `java.project.listSourcePaths`
- `java.project.getSettings`
- `java.project.getClasspaths`
- `java.project.updateClassPaths`
- `java.project.updateSettings`
- `java.project.isTestFile`
- `java.project.getAll`
- `java.project.refreshDiagnostics`
- `java.project.import`
- `java.project.changeImportedProjects`
- `java.project.resolveStackTraceLocation`
- `java.project.upgradeGradle`
- `java.project.resolveWorkspaceSymbol`
- `java.project.updateJdk`
- `java.project.createModuleInfo`

### Hierarchy, runtime, bundles, and support

- `java.navigate.openTypeHierarchy`
- `java.navigate.resolveTypeHierarchy`
- `java.protobuf.generateSources`
- `java.reloadBundles`
- `java.vm.getAllInstalls`
- `java.getTroubleshootingInfo`

`HelixLspBridge::execute_extension_command` verifies an ID against the initialized server's advertised command list before sending it. Code-action commands are not assumed to be server commands: many `java.action.*Prompt` IDs are client UI callbacks.

## JDT LS server-to-client extensions

| Method | Kind | Behavior |
| --- | --- | --- |
| `language/status` | notification | Lifecycle/project status (`Starting`, `Started`, `ServiceReady`, `Error`, and others). |
| `language/actionableNotification` | notification | Severity, message, data, and client/server commands. |
| `language/eventNotification` | notification | Project/classpath/source/build events. |
| `language/progressReport` | notification | Legacy progress; prefer standard work-done progress. |
| `workspace/executeClientCommand` | request | Server asks the editor to execute a client callback and return a value. |
| `workspace/notify` | notification | Generic command-shaped client notification. |

Known domain events are:

| Code | Name | Typical invalidation |
| ---: | --- | --- |
| 100 | `ClasspathUpdated` | Classpaths, launch/test models |
| 200 | `ProjectsImported` | Project list, symbols, tests |
| 210 | `ProjectsDeleted` | Project list, symbols, tests |
| 300 | `IncompatibleGradleJdkIssue` | Runtime/build configuration warning |
| 400 | `UpgradeGradleWrapper` | Gradle wrapper action |
| 500 | `SourceInvalidated` | Open/cached `jdt:` virtual documents |
| 600 | `PreviewFeaturesNotAllowed` | Language-level configuration warning |

Nucleotide decodes numeric and string event representations and tolerates unknown kinds. It displays status/actionable text, maps legacy progress into normal LSP progress, and refuses unadvertised client commands. `executeClientCommandSupport` remains false until command handlers have an allowlist and non-blocking response path.

## Extended initialization capabilities

JDT LS reads these under `initializationOptions.extendedClientCapabilities`:

- Content/editing: `snippetEditSupport`, `classFileContentsSupport`, `resolveAdditionalTextEditsSupport`, `nonStandardJavaFormatting`.
- Generation UI: `overrideMethodsPromptSupport`, `hashCodeEqualsPromptSupport`, `advancedOrganizeImportsSupport`, `generateToStringPromptSupport`, `advancedGenerateAccessorsSupport`, `generateConstructorsPromptSupport`, `generateDelegateMethodsPromptSupport`.
- Refactoring UI: `advancedExtractRefactoringSupport`, `extractInterfaceSupport`, `inferSelectionSupport`, `advancedIntroduceParameterRefactoringSupport`, `moveRefactoringSupport`.
- Project/runtime UI: `advancedUpgradeGradleSupport`, `gradleChecksumWrapperPromptSupport`, `actionableNotificationSupported`, `actionableRuntimeNotificationSupport`.
- Client ownership/callbacks: `clientHoverProvider`, `clientDocumentSymbolProvider`, `executeClientCommandSupport`, `onCompletionItemSelectedCommand`.
- Lifecycle/compatibility: `progressReportProvider`, `shouldLanguageServerExitOnShutdown`, `canUseInternalSettings`, `skipProjectConfiguration`, `skipTextEventPropagation`, `excludedMarkerTypes`.

The adapter currently sets callback/UI capabilities to false. This is deliberate. Enabling a boolean changes what JDT LS emits and can replace a simple edit with a multi-step client-owned flow.

JDT LS also accepts top-level initialization options for `bundles`, `workspaceFolders`, `settings`, `triggerFiles`, and `projectConfigurations`. Nucleotide merges its capability overlay over user options, preserving Java settings and explicitly configured trusted bundles. Initialization-only fields are not returned from `workspace/configuration`.

## Custom code-action kinds

JDT LS extends standard `CodeActionKind` with:

- `source.generate`
- `source.generate.accessors`
- `source.generate.hashCodeEquals`
- `source.generate.toString`
- `source.generate.constructors`
- `source.generate.delegateMethods`
- `source.generate.finalModifiers`
- `source.overrideMethods`
- `source.sortMembers`
- `refactor.extract.function`
- `refactor.extract.constant`
- `refactor.extract.variable`
- `refactor.extract.field`
- `refactor.extract.interface`
- `refactor.move`
- `refactor.assign.variable`
- `refactor.assign.field`
- `refactor.introduce.parameter`
- `refactor.change.signature`
- `quickassist`

The strings are cataloged in `jdtls/protocol.rs`. `refactor.extract.function` is intentionally not renamed to `.method`; clients depend on the historical value.

## Core versus optional components

### Core JDT LS

Core supplies the Java model, Maven/Gradle/Eclipse/invisible-project import, completion, diagnostics, navigation, formatting, hierarchy, inlay hints, semantic tokens, source generation, refactoring, class-file content, source attachment, and FernFlower decompilation.

### Java debug

Debugging is not LSP. The [`microsoft/java-debug`](https://github.com/microsoft/java-debug) bundle contributes `vscode.java.*` launch-resolution commands to JDT LS, while a separate Debug Adapter Protocol process supplies breakpoints, stepping, threads, stack frames, variables, evaluation, and hot-code replacement.

A supportable integration requires:

- a pinned JDT LS + java-debug bundle compatibility set;
- trusted bundle path configuration;
- typed wrappers only for the `vscode.java.*` commands Nucleotide uses;
- a separate DAP client/lifecycle and Java launch UI.

### Java tests

The [`microsoft/vscode-java-test`](https://github.com/microsoft/vscode-java-test) bundle contributes `vscode.java.test.*` discovery, navigation, generation, JUnit-argument, path, and coverage-detail commands. The client still owns the test tree, run/debug orchestration, process/result lifecycle, and coverage UI.

Core `java.project.isTestFile` only classifies a source file; it is not test discovery or execution.

## Nucleotide delivery matrix

| Area | Current foundation | UI/product work still required |
| --- | --- | --- |
| Java startup | Maven/Gradle markers produce `ProjectType::Java`; configured `jdtls` starts eagerly. | JDK/JDT LS installation discovery, unique data-directory policy, recovery UI. |
| Standard basics | Sync, push diagnostics, completion/resolve, hover, navigation, references, symbols, code actions, inlay hints, workspace edits, configuration, messages, and progress use generic paths. | Polish stale-result cancellation and Java-specific acceptance fixtures. |
| Standard advanced | Helix LSP types/capabilities exist. | Signature UI, rename UI, formatting actions, document highlights, code lenses, semantic-token rendering, folding/selection UX, call/type hierarchy panels. |
| Custom outbound | Raw transport plus typed request/notification/command traits; all core IDs centralized. | Application actions and multi-step selection dialogs. |
| Custom inbound | Status, actionable text, domain events, and legacy progress are normalized. | Action buttons, event-driven cache invalidation, client command request handlers. |
| Class files | Typed `java/classFileContents` request exists. | Read-only `jdt:` virtual-document provider, navigation routing, source invalidation refresh. |
| Build/projects | Build/project request and command contracts exist. | Build controls, project/classpath/source-attachment UI. |
| Refactoring/generation | Method and code-action contracts exist. | Candidate dialogs, previews, edit confirmation, follow-up rename. |
| Debug/tests | Bundle command transport is generic. | Versioned bundle registry, DAP, test model, run/debug/results/coverage UI. |

## Recommended rollout order

1. **Finish standard LSP parity first.** Rename, formatting, signature help, semantic tokens, code lenses, and standard call/type hierarchy benefit every language and unlock much of JDT LS without Java-specific code.
2. **Class-file virtual documents.** Implement `jdt:` routing, `java/classFileContents`, read-only buffers, and `SourceInvalidated`. This is essential for library navigation.
3. **Project/runtime/build controls.** Surface service readiness, project import, JDK selection, build status, source paths, classpaths, source attachments, and troubleshooting.
4. **Generation flows.** Add override/accessor/constructor/`toString`/`equals` dialogs one protocol pair at a time, enabling each extended capability only with its UI.
5. **Advanced refactoring.** Build a reusable candidate/preview/apply/follow-up-rename workflow around `getRefactorEdit`, selection inference, and move destinations.
6. **Debug and tests.** Introduce a trusted, pinned bundle registry, then separate DAP and test-controller subsystems.

Every feature should ship with recorded JSON fixtures from the pinned server set, a malformed/additive payload test, cancellation/stale-response coverage, and a multi-file workspace-edit test where applicable.

## Configuration and operational requirements

- Launch one JDT LS process per logical workspace and never share its Eclipse `-data` directory concurrently.
- Keep the server JVM, project runtimes (`java.configuration.runtimes`), and Gradle JVM (`java.import.gradle.java.home`) distinct.
- Send Java settings under the server's expected `java`/`settings.java` structure and preserve unknown keys.
- Let JDT LS own Maven/Gradle import and rebuild policy; do not duplicate build-model parsing in the editor.
- Load only explicitly trusted bundle JARs. Bundle loading executes code inside the JDT LS process.
- Pin and integration-test JDT LS, java-debug, and java-test versions together.
- Use advertised capabilities and command IDs at runtime; version checks are a fallback, not feature detection.
- Keep standard progress as the default. Legacy `language/progressReport` is compatibility input, not the preferred advertised path.

This architecture makes the protocol surface reachable without pretending the IntelliJ-class experience is complete. The remaining work is intentionally product-facing: reusable standard LSP views, safe multi-step Java workflows, virtual class files, and separate debug/test systems rather than more transport special cases.

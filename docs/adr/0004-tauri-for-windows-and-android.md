# 4. Tauri v2 for Windows and Android

Accepted.

## Context

One codebase, two platforms, touch and mouse, portrait and landscape. The thing being
designed is a website, so the editor's canvas and the exported page ideally use the same
rendering engine.

## Decision

Tauri v2. Rust core, web frontend, WebView2 on Windows and Android System WebView on
Android. SolidJS for the interface.

The studio's Rust shell is its own cargo workspace, separate from the core crates.

## Consequences

Good:

- The canvas and the exported site share a rendering engine, so what you design is what
  ships.
- One codebase, two platforms; binaries in the tens of megabytes rather than hundreds.
- Rust gives the geometry kernel, and the same crates serve the CLI and the MCP server.
- Separating the workspace means `cargo test --workspace` runs on a plain machine with no
  GUI libraries — which is where the tests that matter run.

Costs:

- The Android toolchain is fiddly, and the app cannot be built on a machine without a
  system webview. In practice CI is where it builds, and a Linux dev box needs webkit2gtk
  installed.
- Android System WebView versions vary by device. The frontend targets ES2020 and the
  bundle is ~29 KB gzipped, which keeps startup viable on older hardware.
- SolidJS is a smaller ecosystem than React. For an app whose core is a custom canvas
  rather than off-the-shelf components, fine-grained reactivity — one property change
  touching one attribute, with no diff over thousands of nodes — is worth more than the
  component libraries.

## Alternatives considered

**Flutter.** Best-in-class touch and a single high-performance canvas. But it cannot render
web output natively, so every preview would need an embedded WebView anyway and the canvas
would never match the export. Two rendering models forever.

**Electron.** Most mature desktop ecosystem; does not target Android at all.

**PWA plus Capacitor.** Fastest to a prototype; fights the GitHub-release update model, has
weaker filesystem and font access, and gives up the Rust geometry kernel.

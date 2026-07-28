# Baalert

Baalert is a customizable desktop pet that delivers animated reminders without
interrupting your workflow. It is built with Tauri, React, and TypeScript.

## Development

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

Install dependencies and start the desktop app:

```sh
pnpm install
pnpm tauri dev
```

## Releases

Every push and pull request runs the frontend build and Rust tests. Version tags
build draft GitHub releases for macOS Apple Silicon, macOS Intel, and Windows.

Before releasing, update the version in `package.json`, `src-tauri/Cargo.toml`,
and `src-tauri/tauri.conf.json`. Then create and push the matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

Review the generated draft release on GitHub and publish it to make it available
to the in-app updater.

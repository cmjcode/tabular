<div align="center">

# Tabular

Your fast, cross‑platform, multi‑database SQL & NoSQL desktop client – built in Rust.

![Main Window](screenshots/halaman-utama.jpg)

</div>

## 1. About Tabular
Tabular is a lightweight, native, and efficient database client written in Rust using the `eframe`/`egui` stack. It focuses on fast startup, responsive UI, safe concurrency, and a distraction‑free workflow for developers, data engineers, and DBAs. Unlike web/electron clients, Tabular ships as a single native binary with minimal memory footprint while still supporting rich features like autocomplete, multiple drivers, query history, export tools, and self‑update.

## 2. Key Features
* Unified UI for multiple relational & non‑relational databases
* Drivers: PostgreSQL, MySQL/MariaDB, SQLite, SQL Server (TDS), Redis, MongoDB
* Async runtime (Tokio) – non‑blocking query execution
* Multiple query tabs & saved query library (`queries/` dir)
* Query history panel with search & filtering
* Result grid with copy cell / row / full result set
* Export to CSV & XLSX
* Rich value formatting (dates, decimals, JSON, BSON, HEX)
* Connection caching & quick reconnect
* Self‑update (GitHub releases) with semantic version check
* Configurable data directory (supports `TABULAR_DATA_DIR` env var)
* Native file dialogs (via `rfd`)
* Cross‑platform theming via egui
* Sandboxing & macOS notarization ready

## 3. Supported Databases
| Category    | Engines / Protocols |
|-------------|---------------------|
| Relational  | PostgreSQL, MySQL/MariaDB, SQLite, Microsoft SQL Server |
| Document    | MongoDB (with BSON & compression) |
| Key/Value   | Redis (async connection manager) |

> Notes:
> * Microsoft SQL Server uses the `tiberius` (TDS over TLS) driver.
> * Redis connections use pooled async managers.
> * SQLite works in process (file mode) – ensure write permissions.

## 4. Installation

### Option A: Download Prebuilt Release (Recommended)
1. Visit: https://github.com/tabular-id/tabular/releases
2. Download the archive/bundle for your platform:
   * macOS: `.dmg` (notarized) or `.pkg` (if provided)
   * Linux: `.tar.gz` (extract and place binary in `$HOME/.local/bin` or `/usr/local/bin`)
   * Windows (future): Portable `.zip` (planned)
3. (macOS) Drag `Tabular.app` into `/Applications`.
4. Run Tabular.

### Option B: Build From Source
Requirements (general):
* Rust (stable, latest; install via https://rustup.rs)
* Cargo (bundled with rustup)
* Clang/LLVM (for bindgen / some native crates)
* libclang headers available (Linux)
* (Linux) pkg-config, OpenSSL dev packages may be required by transitive dependencies depending on environment

#### Arch Linux
```bash
sudo pacman -Syu --needed base-devel clang llvm pkgconf
export LIBCLANG_PATH=/usr/lib
git clone https://github.com/tabular-id/tabular.git
cd tabular
cargo build --release
```

#### Ubuntu / Debian
```bash
sudo apt update
sudo apt install -y build-essential clang libclang-dev pkg-config
git clone https://github.com/tabular-id/tabular.git
cd tabular
cargo build --release
```

#### macOS
```bash
xcode-select --install   # command line tools
brew install llvm        # (optional) newer clang
git clone https://github.com/tabular-id/tabular.git
cd tabular
cargo build --release
```
If Homebrew LLVM is used:
```bash
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
```

#### Windows (MSVC) – Planned
Install the MSVC toolchain + `rustup toolchain install stable-x86_64-pc-windows-msvc` then:
```powershell
git clone https://github.com/tabular-id/tabular.git
cd tabular
cargo build --release
```

#### Multi‑Architecture / Cross Compilation
Install cross:
```bash
cargo install cross
cross build --target aarch64-apple-darwin --release
```

### Run
```bash
./target/release/tabular
```

### Optional Environment Variables
| Variable | Purpose | Example |
|----------|---------|---------|
| TABULAR_DATA_DIR | Override data directory location | /data/tabular |
| RUST_LOG | Enable logging | RUST_LOG=info ./tabular |

## 5. macOS Notarized / Signed Builds
For distributing outside the Mac App Store:
```bash
export APPLE_ID="your-apple-id@example.com"
export APPLE_PASSWORD="app-specific-password"
export APPLE_TEAM_ID="TEAMID"
export APPLE_BUNDLE_ID="id.tabular.database"
export APPLE_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export NOTARIZE=1
./build.sh macos --deps
```
Staple & verify:
```bash
xcrun stapler staple dist/macos/Tabular-<version>.dmg
spctl -a -vv dist/macos/Tabular.app
codesign --verify --deep --strict --verbose=2 dist/macos/Tabular.app
```
See `macos/Tabular.entitlements` for sandbox/network/file access settings. For App Store distribution use a distribution identity and provisioning profile:
```bash
export APPLE_IDENTITY="Apple Distribution: Your Name (TEAMID)"
export PROVISIONING_PROFILE="/path/Tabular_AppStore.provisionprofile"
make pkg-macos-store
```

## 6. Data Directory (Configurable)
Default locations:
* macOS / Linux: `~/.tabular`
* Windows: `%USERPROFILE%\.tabular`

You can change it inside Preferences (native folder picker) or force it using:
```bash
export TABULAR_DATA_DIR="/custom/path"
./tabular
```
Migration (manual): copy old folder to the new location before switching & restarting.

Contents:
* `preferences.*` – UI & app settings
* `cache.*` – metadata / driver caches
* `queries/` – saved queries
* `history/` – executed query history

## 7. Development Guide
### Project Layout (selected)
```
src/
  main.rs              # Entry point
  window_egui.rs       # UI / egui integration
  editor.rs            # Query editor logic
  editor_autocomplete.rs
  sidebar_*.rs         # Side panels (database, history, queries)
  driver_*.rs          # Database drivers abstraction layers
  export.rs            # CSV / XLSX exporting
  self_update.rs       # Update checker & apply logic
  config.rs            # Preferences & data directory handling
  models/              # Data structures & enums
```

### Quick Start (Dev)
```bash
git clone https://github.com/tabular-id/tabular.git
cd tabular
cargo run
```

### Common Tasks
| Action | Command |
|--------|---------|
| Build debug | `cargo build` |
| Run | `cargo run` |
| Tests (if/when added) | `cargo test` |
| Lint (clippy) | `cargo clippy -- -D warnings` |
| Format | `cargo fmt` |
| Release build | `cargo build --release` |

### Logging
Enable logs (INFO):
```bash
RUST_LOG=info cargo run
```

### Adding a New Driver
1. Create `driver_<engine>.rs`
2. Implement connection open / close / query streaming
3. Add feature flags if optional
4. Register module inside `modules.rs` / relevant factory
5. Update README & supported database table

### Autocomplete
Implemented in `editor_autocomplete.rs` leveraging schema introspection & regex helpers. Future enhancements may include partial AST parsing.

## 8. Core Dependencies (Crates)
| Purpose | Crate(s) |
|---------|----------|
| UI & App Shell | `eframe`, `egui_extras`, `egui_code_editor` |
| Async Runtime | `tokio`, `futures`, `futures-util`, `tokio-util` |
| Relational DB | `sqlx` (postgres, mysql, sqlite features) |
| SQL Server | `tiberius` (TLS via rustls) |
| Redis | `redis` (tokio + connection-manager) |
| MongoDB | `mongodb`, `bson` |
| Data Formats | `serde`, `serde_json`, `chrono`, `rust_decimal`, `hex`, `csv`, `xlsxwriter` |
| File Dialog | `rfd` |
| Update | `reqwest`, `self_update`, `semver` |
| Logging | `log`, `env_logger`, `dotenv` |
| Utility | `dirs`, `regex`, `colorful` |

See `Cargo.toml` for exact versions.

## 9. Contributing
Contributions are welcome: bug fixes, new drivers, UI refinements, performance tweaks. Suggested workflow:
1. Fork & create feature branch.
2. Run `cargo fmt && cargo clippy` before committing.
3. Ensure release build compiles: `cargo build --release`.
4. Open a PR with a concise description & screenshots (if UI changes).

## 10. Troubleshooting
| Issue | Hint |
|-------|------|
| Build fails: clang not found | Install clang / set `LIBCLANG_PATH` |
| Cannot connect (TLS errors) | Verify server certificates / network reachability |
| SQLite file locked | Close other processes; check permissions |
| UI freeze during long query | Future improvement: streaming pagination (in progress) |

## 11. Roadmap (High‑Level)
* Windows build & signing
* Query formatting & beautifier
* Result pagination for large sets
* Connection grouping & tags
* Plugin / extension scripting layer
* Secure secrets storage integration (Keychain / KWallet / Credential Manager)

## 12. License
Pending (No LICENSE file committed yet). Until a license is added, usage is implicitly restricted—please open an issue to clarify before redistribution.

## 13. Acknowledgements
Built with the Rust community ecosystem. egui & sqlx projects are especially instrumental.

---
Made with Rust 🦀 for people who love fast, native tools.


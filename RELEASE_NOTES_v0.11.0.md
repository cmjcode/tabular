# 🚀 Tabular Release Notes: v0.10.5 → v0.11.0

We are excited to announce **v0.11.0**, headlined by a major overhaul of the **Built-in HTTP Client** — now with a full request **Collections** workflow and one-click imports from **Postman** and **Yaak** — plus a rebuilt **cross-platform auto-update pipeline** with dedicated Windows CI builds.

---

## 🌟 Highlights & Major Features

### 🌐 HTTP Client Collections
* **Collections Sidebar**: Brand-new sidebar tree (`sidebar_collection.rs`) for organizing HTTP requests into folders/collections, separate from the database connection tree.
* **Import from Postman**: Import existing Postman collections directly into Tabular's HTTP Client.
* **Import from Yaak**: Import existing Yaak workspaces/requests directly into Tabular.
* **Rename Requests**: Rename saved HTTP requests via a dedicated modal dialog.
* **Connection & Tab Management**: Create new HTTP request tabs bound to a workspace, save requests in place, and confirm before deleting.
* **Smarter Auto-Naming**: New requests are now auto-named from their endpoint/URL instead of a generic placeholder.
* **Context Menu & Confirmation Fixes**: Reworked right-click context menu behavior and delete/action confirmation popups for the collections tree.
* **UI Polish**: Toast notification positioning updated, and the Databases sidebar tab layout was cleaned up and rebalanced alongside the new HTTP Client panel.

### 🔄 Cross-Platform Auto-Update
* **Rebuilt Update Engine**: `auto_updater.rs` significantly expanded with more robust chunked download, staging, and apply logic across platforms.
* **Windows Update Reliability**: Automatic cleanup of leftover `.exe.old` files from previous updates on startup.
* **Self-Update Overhaul**: `self_update.rs` update-check/apply flow hardened and expanded.
* **Windows CI Build Workflow**: New/updated GitHub Actions pipeline (`build.yml`) and `build_windows.ps1` script for producing Windows release builds.

### 📦 Dependency Updates
* `redis`: `1.4.1` → `1.5.0`
* `async-trait`: `0.1.91` → `0.1.92`

---

## 📦 Detailed Changelog

### ✨ New Features & Enhancements
* **HTTP Collection Storage** (`http_collection.rs`, new): Data model and persistence layer for HTTP request collections/workspaces.
* **Postman Import**: Parse and import Postman collection files into the new collections sidebar.
* **Yaak Import**: Parse and import Yaak workspace files into the new collections sidebar.
* **Request Rename Dialog**: Rename any saved HTTP request in place via modal.
* **HTTP Tab Creation Helper**: New `create_new_http_tab` in `editor.rs` to spin up a pre-configured HTTP Client tab bound to a connection.
* **Save/Update Flow for HTTP Tabs**: `save_current_tab` now saves or updates the HTTP request state directly (via `save_or_update_http_tab`) and refreshes workspaces in place instead of always reopening a save dialog.
* **Query Sidebar Extraction** (`sidebar_query.rs`, new): Query-related sidebar logic split out from `sidebar_tree.rs`/`search.rs` for a cleaner separation between the SQL and HTTP sidebars.
* **Delete Confirmation Popups**: New confirmation dialogs before deleting HTTP requests/collections.

### 🐛 Fixes
* **Context Menu**: Fixed incorrect/broken right-click context menu behavior in the collections sidebar.
* **Confirmation Popup**: Fixed the delete/action confirmation popup not behaving correctly.
* **Request Naming**: Fixed new requests not using the endpoint URL for their default name.
* **Databases Sidebar UI**: Fixed layout/rendering issues in the Databases tab after introducing the HTTP Client panel alongside it.
* **Toast Position**: Fixed toast notifications overlapping/misplaced after HTTP client changes.

### 💅 Refactoring & Code Quality
* Split query-sidebar rendering out of `sidebar_tree.rs` and `search.rs` into a dedicated `sidebar_query.rs`.
* Reworked `app_impl.rs` tab/panel rendering to accommodate the new Collections sidebar alongside the Databases sidebar.
* Cleaned up `.gitignore` and dependency versions bumped in `Cargo.toml`/`Cargo.lock`.

---

### Summary of Changes

```
v0.10.5  ──►  v0.11.0 (HTTP Client Collections, Postman/Yaak Import, Cross-Platform Auto-Update, Windows CI)
```

---

## 📥 Full Commit List
- `feat`: add rename functionality for HTTP requests via modal dialog
- `fix`: confirmation popup
- `fix`: context menu
- `feat`: implement HTTP connection management (tab creation, request saving, delete confirmation, toast positioning)
- `fix`: request naming to use endpoint
- `fix`: Databases sidebar UI
- `fix`: Databases tab UI/layout
- `feat`: import from Postman
- `feat`: import from Yaak
- `feat`: implement cross-platform auto-update logic and CI build workflow for Windows releases
- chore: dependency version upgrades (`redis`, `async-trait`)

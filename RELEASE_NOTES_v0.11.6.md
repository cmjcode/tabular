

We are thrilled to announce **v0.11.6**, a major update packed with **blazing-fast asynchronous startup optimizations**, comprehensive **Teams & Collaboration Management**, **Drag-and-Drop** organization for HTTP collections, multi-language **Code Export ("Copy as Code")**, and essential UI & reliability enhancements.

---

## 🌟 Highlights & Major Features

### ⚡ Blazing-Fast Startup & Non-Blocking Async Architecture
* **Asynchronous Database Bootstrap**: Database initialization and schema migration now execute non-blockingly in a background thread pool, drastically slashing initial app freeze.
* **Parallel Background Loading**: HTTP request collections, query trees, sync accounts, and connection icons load concurrently in background workers.
* **Instant Window Readiness**: First interactive frame renders in ~180ms!
* **Optimized Tree Toggle Logic**: Custom toggle state handling for sidebar folders replacing heavy state recalculations.
* **Built-in Startup Diagnostics**: Fine-grained `[STARTUP-TIMER]` logging to trace subsystem launch benchmarks.

### 👥 Teams Management & Collaboration Overhaul
* **Teams & Collab Sidebar Sub-Tabs**: Dedicated navigation tabs in the sidebar for switching smoothly between **Teams** and **Collaboration**.
* **Team & Member Management**: Create teams, list rooms, and manage team members via dedicated modal dialogs and API integration.
* **Team Folder Sharing**: Direct UI and API integration for sharing request folders and resources with specific teams.
* **Persistent Teams Cache**: Fast local caching layer (`sync_teams_cache.rs`) to accelerate team list and room rendering without redundant network roundtrips.
* **HTTP Collection Cloud Sync**: Background synchronization for local HTTP request collections via API client and worker routines.
* **CRDT Engine Upgrade**: Collaboration protocol upgraded to `yrs 0.27` with modern cryptographic crate updates.

### 🌐 HTTP Client: Drag-and-Drop & Multi-Language Code Export
* **Drag-and-Drop Support**: Easily reorder and move HTTP requests and folders between workspaces and parent folders via drag-and-drop.
* **"Copy as Code" Export**: Export any HTTP request into clean, syntax-highlighted code snippets in multiple languages (cURL, Python, JavaScript/Fetch, Rust, Go, PHP, etc.) via `http_code_export.rs`.
* **Smart Tab Switching**: Intelligently switches to an already open tab for a request instead of opening unwanted duplicate tabs.
* **Output Scroll Improvements**: Smooth scrolling behavior for response bodies in the HTTP client inspector.

### 🗄️ Database Sidebar & Structure Editor Improvements
* **Sidebar Databases Redesign**: Polished visual aesthetics, typography, and spacing for the database connection tree.
* **Add Column & Index Fixes**: Fixed dropdown combobox behavior when adding new columns or index definitions in the table structure editor.

---

## 📦 Detailed Changelog

### ✨ New Features & Enhancements
* **Async Startup Architecture**:
  * Implement background database initialization with SQLite pool (`d828b62`).
  * Implement asynchronous HTTP collection loading and startup diagnostics (`0be67db`).
  * Migrate sync account loading to background bootstrap (`6aeba46`).
* **Teams & Collaboration**:
  * Introduce sub-tab navigation for Teams and Collaboration in sidebar (`f84725d`, `df18eaa`).
  * Add support for managing teams and team members with dedicated modal dialog (`50c3b14`, `d70d2fa`).
  * Add API and UI integration for listing/creating team rooms and sharing folders (`c470ca5`, `cb1f005`).
  * Implement persistent teams cache (`a949db4`).
  * Implement background sync for local HTTP collections (`a90c4bf`).
  * Add login & authentication flow enhancements for team sync (`c14be06`).
* **HTTP Client**:
  * Implement drag-and-drop for moving HTTP requests and folders (`e85ea81`).
  * Add "Copy as Code" code export supporting multiple programming languages with syntax colors (`7057964`, `d84e026`).
  * Streamline tab switching to prioritize existing active request tabs (`f6bc28b`).
* **Databases & Schema**:
  * Improve design, styling, and dialog rendering for Databases sidebar (`98da139`).

### 🐛 Fixes & Polish
* **Folder Sidebar Toggle**: Replace `CollapsingHeader` with lightweight custom toggle state management (`7346b8f`).
* **Add Room UX**: Enable '+' button dynamically after typing a room name (`be242a4`, `b1ea325`).
* **Table Structure**: Fix combobox selection when adding columns or indexes (`6bd8ba3`).
* **HTTP Client Output**: Fix scroll output handling in response viewer (`215db95`).

### 🔧 Dependencies & Maintenance
* Upgrade `yrs` to `0.27` and update cryptographic dependencies across collaboration and sync crates (`3bc25d5`).

---

### Summary of Changes

```
v0.11.0  ──►  v0.11.6 (Async Startup ~180ms, Teams & Collab Overhaul, Drag-and-Drop, Code Export, yrs 0.27)
```

---

## 📥 Full Commit List

- `0fc57c9` release v0.11.6
- `c2aa04a` Ok load lebih cepet
- `7346b8f` refactor: replace CollapsingHeader with custom toggle logic for folder sidebar state management
- `6aeba46` refactor: migrate sync account loading from initialization to background database bootstrap and remove unused preference handling
- `d828b62` feat: implement background database initialization and schema setup with SQLite pool
- `0be67db` feat: implement asynchronous HTTP collection loading and add comprehensive startup timing diagnostics
- `e85ea81` feat: implement drag-and-drop functionality for moving HTTP requests and folders between workspaces and folders
- `3bc25d5` chore: update core dependencies and migrate collaboration modules to yrs 0.27 and updated cryptographic crates
- `a949db4` feat: implement persistent team cache and enhance folder sharing UI in teams sync module
- `1aa77a2` ok
- `d70d2fa` refactor: migrate inline team member addition to a dedicated modal dialog
- `f84725d` feat: introduce sub-tab navigation for Teams and Collaboration sections in the sidebar
- `df18eaa` feat: update collaborations sidebar to include teams section and sidebar-specific collab rendering
- `cb1f005` feat: add UI and API integration for sharing folders with teams
- `50c3b14` feat: add support for managing teams and team members via API and Egui UI state
- `c470ca5` feat: add methods to list and create team rooms in the API client
- `a90c4bf` feat: implement sync logic for local HTTP request collections via API client and background tasks
- `c14be06` ok phase 2
- `be242a4` button + to add room, will actived after type a room name
- `b1ea325` fixed add new room
- `98da139` improve design sidebar databases
- `6bd8ba3` fixed combobx on add column or index
- `d84e026` ok copy as code
- `7057964` colorfull code
- `215db95` fixed scoll output
- `f6bc28b` refactor: optimize tab switching logic to prioritize existing request-specific tabs and streamline new tab creation

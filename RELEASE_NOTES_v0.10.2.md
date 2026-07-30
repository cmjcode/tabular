# 🚀 Tabular Release Notes: v0.10.0 → v0.10.2

We are excited to announce **v0.10.2**, featuring major improvements to **Cloud Sync**, **Collaboration Rooms**, **Connection Reliability (SSH & Fast-Fail)**, and a comprehensive **UI Design System Redesign**.

---

## 🌟 Highlights & Major Features

### ☁️ Cloud Sync & OAuth Authentication
* **Automated OAuth Authentication**: Integrated seamless OAuth loopback authentication flow with centralized background sync execution.
* **Persistent Sync Server Config**: Configurable cloud sync endpoint defaulting to `api.tabular.id`.
* **Account Management UI**: User profile management displaying user avatars and wrapped API envelope response handling.

### 👥 Real-time Collaboration Enhancements
* **Collaboration UI Redesign**: Modernized collaboration layout with active session expiration handling and status indicators.
* **Room Management**: Added room deletion capability and automatic room list refreshing in the collaboration sidebar.

### 🔌 Database Connectivity & SSH Tunneling
* **SSH Tunnel Support**: Full SSH-aware connection tunneling support with optimized connection timeouts.
* **TCP Fast-Fail Verification**: Pre-flight TCP reachability checks before initiating full database connections to avoid long hangs on unreachable hosts.
* **Background Pool & Retry**: Non-blocking background connection pool creation with retry options for failed database connections.
* **Structure Refresh**: Automatic background structure refresh for active database connections.

### 🎨 Design System & Settings Redesign
* **Modernized Settings Dialog**: Completely redesigned Settings menu featuring custom visual styling, icons, and keyboard shortcut indicators.
* **Custom Tab Bar Rendering**: Redesigned tab bar rendering with smooth hover states, custom active tabs, and consistent tab switching logic.
* **Unified Button Helpers**: Standardized button components across the application using new design system helpers (`render_custom_tab`).
* **Schema Visibility Control**: Dynamic top bar schema selector that automatically hides for non-schema-supporting databases.

---

## 📦 Detailed Changelog by Release

### 🔹 Version 0.10.2

#### ✨ New Features & Enhancements
* **Collaboration Room Deletion**: Delete collaboration rooms directly from the sidebar UI and enjoy instant room list refreshes.
* **Collaboration Session Expiration**: Gracefully handle expired collaboration sessions with status alerts and renewal actions.
* **Redesigned Settings Menu & Tabs**: Brand new custom rendering for settings tabs and menus with polished styling, icons, and shortcuts.
* **Database Schema Support Checks**: Restrict schema dropdown visibility to compatible engines (e.g., PostgreSQL/MSSQL) and adjust top-bar dimensions accordingly.
* **SSH Tunneling & Background Refresh**: Integrated SSH tunnel management with background database structure sync.
* **TCP Reachability Pre-check**: Fast-fail check prevents connection delays when target servers are unreachable.
* **Schema Drop & Data Type Helpers**: Structure view support for database-specific data types and schema drop actions.
* **OAuth Loopback & Sync Runner**: Standardized task spawning and loopback server for Cloud Sync OAuth flow.

#### 💅 Refactoring & Code Quality
* **UI Design System Standardization**: Replaced raw button components with unified custom design system helpers.
* **Modernized Rust Idioms**: Cleaned up UI conditional rendering and updated documentation comments to inner attributes.

---

### 🔹 Version 0.10.1

#### 🔌 Connections & Cache Reliability
* **Background Connection Pools**: Connection pool creation runs asynchronously in background tasks to keep UI responsive.
* **Cache Safety**: Deferred SQLite cache clearance until after pool creation succeeds, preventing cache loss during failed connections.
* **Retry Support**: Added manual retry triggers for failed database connections.
* **Folder-Prefixed Display Names**: Added `display_name` support in `ConnectionConfig` for cleaner folder path labeling in connection trees.

#### 🎨 Sidebar & Navigation Improvements
* **Simplified Connection Rows**: Removed text badges for a cleaner, unified sidebar look.
* **MSSQL Visibility**: Updated MSSQL icon asset and increased icon sizes for improved visibility.
* **ApiHttp Expansion Fix**: Fixed an issue where `ApiHttp` connection nodes auto-expanded unintentionally in the sidebar tree.

---

### Summary of Changes

```
v0.10.0  ──►  v0.10.1 (Connection Pools, Cache Safety, UI Fixes)
         ──►  v0.10.2 (Cloud Sync, OAuth, Collaboration, SSH Tunnel, Settings Redesign)
```

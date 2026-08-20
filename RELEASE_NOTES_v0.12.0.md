

We are excited to announce **v0.12.0**, a security-focused release built around **end-to-end encrypted Cloud Sync** — connection credentials and HTTP client secrets are now encrypted on your device before they ever reach `tabular-server`. This release also ships several reliability fixes for query connections and the SQLite metadata cache.

---

## 🌟 Highlights & Major Features

### 🔐 End-to-End Encrypted Cloud Sync (Zero-Knowledge Vault)
* **Sync Passphrase**: a new secret, separate from your OAuth login, set in **Settings → Sync & Account**. It derives a Key-Encryption-Key via **Argon2id** which unwraps a random per-account `AccountKey`; only that key can decrypt your synced connections and HTTP client secrets — the server never sees it.
* **AES-256-GCM everywhere**: connection credentials and HTTP client secrets (bearer tokens, API keys, basic-auth passwords) are encrypted client-side with AES-256-GCM before upload. `tabular-server` stores ciphertext it cannot read.
* **Team-shared folders stay zero-knowledge too**: each team gets its own `TeamKey`, sealed individually to every member's X25519 public key (anonymous "sealed box" encryption), so the server can relay key material without ever being able to open it.
* **Recovery code**: shown once when your vault is created — the only way back in if you forget your Sync Passphrase, since the server has no way to recover it for you.
* **Local secrets untouched**: on-device credential storage keeps its own separate device-local master key (backed by the OS keychain where available) and is unaffected by this change.
* **New vault setup / unlock UI** (`ui_vault_setup.rs`) walks first-time users through creating a passphrase and displaying the recovery code, and returning users through unlocking their vault.
* **Server-side version gate**: `tabular-server` now rejects sync requests (connections, HTTP requests, vault keys, teams) from clients older than a configurable `MIN_CLIENT_VERSION`, with a clear "please update" error instead of a silent failure — `/health` and login remain reachable from any client version.

### 🛠️ Reliability & Connection Handling
* **Query pool wait: cancel & timeout**: waiting for a connection from the pool can now be cancelled cleanly (closing the wait dialog properly cancels the in-flight connection attempt) and now times out after 30 seconds instead of hanging indefinitely, surfacing a clear error in the query tab.
* **SQLite metadata cache self-healing**: metadata commits now detect SQLite corruption/malformed-database errors, run a WAL checkpoint + `REINDEX`, and retry once automatically instead of failing outright.
* **Metadata cache conflict handling**: database/table/column/index cache writes switched from `INSERT OR REPLACE` to explicit `ON CONFLICT` upserts, plus in-memory deduplication of tables, columns, and indexes before writing — avoiding duplicate rows and unnecessary primary-key churn.
* **App icon set synchronously** during native window initialization instead of on a background thread, removing a class of icon-not-set race conditions on startup.

---

## 📦 Detailed Changelog

### ✨ New Features
* **Cloud Sync E2E Encryption**:
  * Add zero-knowledge vault crypto module — Argon2id KDF, AES-256-GCM key wrapping, X25519 sealed-box team key sharing (`src/sync/vault_crypto.rs`).
  * Add Sync Passphrase setup, unlock, and recovery-code UI in Settings → Sync & Account (`src/sync/ui_vault_setup.rs`).
  * Add vault-key and key-envelope API client methods: `get_vault_keys`, `put_vault_keys`, `list_public_keys`, `get_my_key_envelope`, `list_pending_key_grants`, `put_key_envelopes` (`src/sync/api_client.rs`).
  * Encrypt/decrypt connection and HTTP request payloads in the sync pipeline before upload / after download (`src/sync/sync_connections.rs`, `src/sync/sync_http_requests.rs`, `src/sync/vault_sync.rs`).
  * Add legacy plaintext-compat decryption path for rows synced before this feature existed, migrated lazily on first unlock (`src/sync/legacy_crypto.rs`).
  * Wire vault unlock/lock state into the sync background loop and app init (`src/window_egui/sync_tick.rs`, `src/window_egui/init.rs`, `src/window_egui/mod.rs`).
  * Document the E2E threat model and known limitations in `SECURITY.md`.

### 🐛 Fixes & Polish
* **Query Pool Waiting**: implement connection cancellation, a 30-second timeout, and proper error-state propagation while waiting for a pooled connection (`fix: implement connection cancellation, timeout logic...`).
* **SQLite Metadata Cache**: add corruption self-healing (checkpoint + reindex + retry), `ON CONFLICT` upserts, and deduplication for tables/columns/indexes during MySQL metadata staging (`fix: improve SQLite metadata caching robustness...`).
* **App Icon**: set the native app icon directly during window initialization instead of via an async background thread (`refactor: remove asynchronous background thread for app icon...`).

### 🔧 Server-Side (tabular-server, supporting this release)
* Add `MIN_CLIENT_VERSION` config and `require_min_client_version` middleware to gate sync routes by client version (`src/middleware/version_gate.rs`, `src/config.rs`).
* Add vault key storage/retrieval and team key-envelope endpoints (`src/vault/`, `src/teams/handler.rs`).
* Add corresponding schema for wrapped account keys, X25519 public keys, and per-member team key envelopes (`src/db/schema.sql`).

---

### Summary of Changes

```
v0.11.6  ──►  v0.12.0 (End-to-End Encrypted Cloud Sync, connection cancel/timeout, SQLite cache self-healing)
```

---

## 📥 Full Commit List

- `8df21db` ok E2E
- `6c91024` refactor: remove asynchronous background thread for app icon and set directly during native options initialization
- `30a3463` fix: implement connection cancellation, timeout logic, and error state handling for query pool waiting
- `c0c9c26` fix: improve SQLite metadata caching robustness with conflict handling, deduplication, and corruption self-healing.
- `ae75abf` docs: add release notes for v0.11.6 covering async startup, team collaboration, and UI enhancements

---

## ⚠️ Upgrade Notes

* After upgrading, you'll be prompted to set a **Sync Passphrase** the first time you open Settings → Sync & Account. **Save the recovery code shown at that point** — it is the only fallback if you forget the passphrase, and the server cannot recover it for you.
* If your `tabular-server` deployment sets `MIN_CLIENT_VERSION`, older clients will be blocked from syncing (with a clear upgrade message) until they update.
* Query URLs are not encrypted (only headers/body/auth are) — avoid placing secrets directly in query strings.

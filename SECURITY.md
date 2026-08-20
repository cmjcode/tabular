# Security Policy

## Cloud Sync: End-to-End Encryption

When Tabular Cloud Sync is enabled, connection credentials, HTTP client
secrets (bearer tokens, API keys, basic-auth passwords), saved queries, and
query history are encrypted **on this device, before upload**, and
tabular-server only ever stores ciphertext it cannot read.

- **Sync Passphrase** — set separately from your OAuth login (Settings →
  Sync & Account). Derives a Key-Encryption-Key via Argon2id
  (`src/sync/vault_crypto.rs`), which unwraps a random AccountKey used to
  AES-256-GCM-encrypt your own connections, HTTP requests, saved queries, and
  query history. Query history has no folder/Team-share concept, so it's
  always encrypted with the AccountKey, never a Team key.
- **Team-shared folders** use a separate Team key, sealed individually to
  each member's X25519 public key (anonymous "sealed box" encryption) so
  the server relays it without ever being able to open it.
- **Recovery code** — shown once when you create your vault. It's the only
  way back in if you forget your Sync Passphrase; we cannot recover it for
  you (zero-knowledge design — the server never has enough information to).
- Local storage of credentials (`src/secrets.rs`) is separate and unaffected:
  it's encrypted at rest with a device-local master key, backed by the OS
  keychain where available.

**Known limitations:** the request URL itself (as opposed to headers/body/
auth), and a saved query's/history item's `name` / `folder_path` /
`connection_name`, are not encrypted, since they're used for search/display/
Team-folder sharing; avoid putting secrets in query strings or query names.
Rows synced before this feature existed are migrated lazily on first unlock
after upgrading (query history has no update endpoint, so legacy history rows
are simply read as plaintext until they age out — not migrated in place),
not retroactively rewritten on the server. Local device compromise is outside
this threat model — E2E protects data in transit and at rest on the server,
not on a compromised client.

## Supported Versions

Use this section to tell people about which versions of your project are
currently being supported with security updates.

| Version | Supported          |
| ------- | ------------------ |
| 5.1.x   | :white_check_mark: |
| 5.0.x   | :x:                |
| 4.0.x   | :white_check_mark: |
| < 4.0   | :x:                |

## Reporting a Vulnerability

Use this section to tell people how to report a vulnerability.

Tell them where to go, how often they can expect to get an update on a
reported vulnerability, what to expect if the vulnerability is accepted or
declined, etc.

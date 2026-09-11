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

Security updates are actively provided for the current release series:

| Version | Supported          |
| ------- | ------------------ |
| 0.16.x | :white_check_mark: |
| < 0.16.0 | :x:                |

## Reporting a Vulnerability

We take the security and privacy of Tabular and its users seriously. If you discover a security vulnerability, please report it responsibly:

1. **GitHub Security Advisories (Recommended)**:  
   Submit a private report directly at [GitHub Security Advisories](https://github.com/tabular-id/tabular/security/advisories/new).
2. **Email**:  
   Contact the core team directly at [`security@tabular.id`](mailto:security@tabular.id) or [`support@tabular.id`](mailto:support@tabular.id).

### Response Expectations
- **Initial Acknowledgment**: Within 48 hours of submission.
- **Triage & Status Updates**: Regular updates within 3–5 business days as the issue is investigated and remediated.
- **Coordinated Disclosure**: We kindly request that you do not publicly disclose the issue until a patch has been released. Credit will be acknowledged in release notes if desired.


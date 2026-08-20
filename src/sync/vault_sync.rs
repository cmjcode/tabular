//! Orchestrates the E2E vault across the network: which key encrypts a given
//! resource, and how a Team's shared symmetric key gets distributed to (and
//! revoked from) its members without the server ever holding it in the clear.
//!
//! See `sync::vault_crypto` for the actual cryptography; this module is the
//! plumbing that calls it at the right time with data fetched via `ApiClient`.

use log::{info, warn};
use std::collections::HashMap;

use super::api_client::{
    ApiClient, KeyEnvelopeItemReq, PutKeyEnvelopesReq, RemoteSharedFolder,
};
use super::vault_crypto::{self, SymKey, UnlockedVault};

/// Resolve which key should encrypt/decrypt a resource filed under
/// `folder_path`. `None` means the folder is shared to a Team whose key we
/// don't have unlocked yet — callers must treat that as "can't sync this
/// item right now" rather than silently falling back to the personal key
/// (that would desync it from the rest of the Team).
pub fn resolve_key_for_folder<'a>(
    account_key: &'a SymKey,
    team_keys: &'a HashMap<String, SymKey>,
    shared_folders: &[RemoteSharedFolder],
    resource_type: &str,
    folder_path: &str,
) -> Option<&'a SymKey> {
    match shared_folders
        .iter()
        .find(|f| f.resource_type == resource_type && f.folder_path == folder_path)
    {
        Some(folder) => team_keys.get(&folder.team_id),
        None => Some(account_key),
    }
}

/// Unseal this user's key envelope for every Team they belong to, building
/// the working `team_id -> TeamKey` map used by [`resolve_key_for_folder`].
/// Teams with no envelope yet for this user (still pending a grant from
/// another online member) are silently skipped — their resources just stay
/// un-syncable until a grant lands, not corrupted or misencrypted.
pub async fn unlock_all_team_keys(
    client: &ApiClient,
    token: &str,
    vault: &UnlockedVault,
    team_ids: &[String],
) -> HashMap<String, SymKey> {
    let mut out = HashMap::new();
    for team_id in team_ids {
        match client.get_my_key_envelope(token, team_id).await {
            Ok(Some(envelope)) => match vault_crypto::unwrap_team_key(vault, &envelope.wrapped_team_key) {
                Ok(key) => {
                    out.insert(team_id.clone(), key);
                }
                Err(e) => warn!("[vault_sync] Failed to unseal Team {} key: {}", team_id, e),
            },
            Ok(None) => {
                info!("[vault_sync] No key envelope yet for Team {} — waiting for a grant", team_id);
            }
            Err(e) => warn!("[vault_sync] Failed to fetch key envelope for Team {}: {}", team_id, e),
        }
    }
    out
}

/// Ensure the caller has a Team vault key, generating one if this is the
/// very first time anything is shared with this Team. Grants themselves an
/// envelope immediately. Returns the (now locally known) TeamKey.
pub async fn ensure_own_team_key(
    client: &ApiClient,
    token: &str,
    my_user_id: &str,
    vault: &UnlockedVault,
    team_id: &str,
    team_keys: &mut HashMap<String, SymKey>,
) -> anyhow::Result<SymKey> {
    if let Some(key) = team_keys.get(team_id) {
        return Ok(key.clone());
    }

    if let Some(envelope) = client.get_my_key_envelope(token, team_id).await? {
        let key = vault_crypto::unwrap_team_key(vault, &envelope.wrapped_team_key)
            .map_err(|e| anyhow::anyhow!("failed to unseal existing Team key: {e}"))?;
        team_keys.insert(team_id.to_string(), key.clone());
        return Ok(key);
    }

    // First time anything is shared with this Team: mint a key and self-grant.
    let team_key = SymKey::generate();
    let my_pub_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, vault.x25519_public_bytes);
    let sealed = vault_crypto::wrap_team_key(&my_pub_b64, &team_key)
        .map_err(|e| anyhow::anyhow!("failed to seal Team key for self: {e}"))?;

    client
        .put_key_envelopes(
            token,
            team_id,
            &PutKeyEnvelopesReq {
                envelopes: vec![KeyEnvelopeItemReq {
                    user_id: my_user_id.to_string(),
                    wrapped_team_key: sealed,
                }],
            },
        )
        .await?;

    team_keys.insert(team_id.to_string(), team_key.clone());
    info!("[vault_sync] Minted a new Team vault key for {}", team_id);
    Ok(team_key)
}

/// Grant the Team's key to every member who doesn't have an envelope yet.
/// Safe to call opportunistically (e.g. every sync tick, and right after a
/// folder share/invite) — it's a no-op once everyone already has one.
/// Requires `team_key` to already be unlocked locally (i.e. the caller has
/// been a member long enough to have their own envelope already).
pub async fn grant_pending_team_key_envelopes(
    client: &ApiClient,
    token: &str,
    team_id: &str,
    team_key: &SymKey,
) -> anyhow::Result<usize> {
    let pending = client.list_pending_key_grants(token, team_id).await?;
    if pending.is_empty() {
        return Ok(0);
    }

    let mut envelopes = Vec::with_capacity(pending.len());
    for member in &pending {
        match vault_crypto::wrap_team_key(&member.x25519_public_key, team_key) {
            Ok(sealed) => envelopes.push(KeyEnvelopeItemReq {
                user_id: member.user_id.clone(),
                wrapped_team_key: sealed,
            }),
            Err(e) => warn!(
                "[vault_sync] Failed to seal Team {} key for pending member {}: {}",
                team_id, member.user_id, e
            ),
        }
    }

    if envelopes.is_empty() {
        return Ok(0);
    }

    let granted = envelopes.len();
    client
        .put_key_envelopes(token, team_id, &PutKeyEnvelopesReq { envelopes })
        .await?;
    info!("[vault_sync] Granted Team {} key to {} pending member(s)", team_id, granted);
    Ok(granted)
}

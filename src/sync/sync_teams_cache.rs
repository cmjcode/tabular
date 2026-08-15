//! SQLite Cache for Teams, Members, and Shared Folders.
//! Offline-first: loads local SQLite cache on startup, updates cache on remote sync.

use std::collections::HashMap;
use sqlx::SqlitePool;
use log::warn;

use super::api_client::{RemoteTeam, RemoteTeamMember, RemoteSharedFolder};

/// Initialize SQLite tables for Teams cache in connections.db
pub async fn init_teams_cache_tables(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS teams_cache (
            id TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT ''
        )
        "#
    ).execute(pool).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS team_members_cache (
            team_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            email TEXT NOT NULL,
            display_name TEXT,
            avatar_url TEXT,
            username TEXT,
            phone TEXT,
            role TEXT NOT NULL DEFAULT 'member',
            joined_at TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (team_id, user_id)
        )
        "#
    ).execute(pool).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS team_shared_folders_cache (
            id TEXT PRIMARY KEY,
            team_id TEXT NOT NULL,
            resource_type TEXT NOT NULL,
            folder_path TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT ''
        )
        "#
    ).execute(pool).await?;

    Ok(())
}

/// Load all cached teams from SQLite
pub async fn load_teams_from_cache(pool: &SqlitePool) -> Vec<RemoteTeam> {
    let res = sqlx::query_as::<_, (String, String, String, Option<String>, String, String)>(
        "SELECT id, owner_id, name, description, created_at, updated_at FROM teams_cache ORDER BY name"
    )
    .fetch_all(pool)
    .await;

    match res {
        Ok(rows) => rows.into_iter().map(|(id, owner_id, name, description, created_at, updated_at)| {
            RemoteTeam {
                id,
                owner_id,
                name,
                description,
                created_at,
                updated_at,
            }
        }).collect(),
        Err(e) => {
            warn!("[teams_cache] Failed to load teams cache: {}", e);
            Vec::new()
        }
    }
}

/// Load cached team members for all teams from SQLite
pub async fn load_team_members_from_cache(pool: &SqlitePool) -> HashMap<String, Vec<RemoteTeamMember>> {
    let mut map: HashMap<String, Vec<RemoteTeamMember>> = HashMap::new();
    let res = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, String, String)>(
        "SELECT team_id, user_id, email, display_name, avatar_url, username, phone, role, joined_at FROM team_members_cache"
    )
    .fetch_all(pool)
    .await;

    if let Ok(rows) = res {
        for (team_id, user_id, email, display_name, avatar_url, username, phone, role, joined_at) in rows {
            let member = RemoteTeamMember {
                user_id,
                email,
                display_name,
                avatar_url,
                username,
                phone,
                role,
                joined_at,
            };
            map.entry(team_id).or_default().push(member);
        }
    }
    map
}

/// Load all cached shared folders from SQLite
pub async fn load_shared_folders_from_cache(pool: &SqlitePool) -> Vec<RemoteSharedFolder> {
    let res = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT id, team_id, resource_type, folder_path, created_at FROM team_shared_folders_cache"
    )
    .fetch_all(pool)
    .await;

    match res {
        Ok(rows) => rows.into_iter().map(|(id, team_id, resource_type, folder_path, created_at)| {
            RemoteSharedFolder {
                id,
                team_id,
                resource_type,
                folder_path,
                created_at,
            }
        }).collect(),
        Err(e) => {
            warn!("[teams_cache] Failed to load shared folders cache: {}", e);
            Vec::new()
        }
    }
}

/// Save entire teams list to cache (full refresh / overwrite)
pub async fn save_teams_cache(pool: &SqlitePool, teams: &[RemoteTeam]) {
    let _ = sqlx::query("DELETE FROM teams_cache").execute(pool).await;
    for t in teams {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO teams_cache (id, owner_id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(&t.id)
        .bind(&t.owner_id)
        .bind(&t.name)
        .bind(&t.description)
        .bind(&t.created_at)
        .bind(&t.updated_at)
        .execute(pool)
        .await;
    }
}

/// Save single team to cache
pub async fn save_single_team_cache(pool: &SqlitePool, t: &RemoteTeam) {
    let _ = sqlx::query(
        "INSERT OR REPLACE INTO teams_cache (id, owner_id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&t.id)
    .bind(&t.owner_id)
    .bind(&t.name)
    .bind(&t.description)
    .bind(&t.created_at)
    .bind(&t.updated_at)
    .execute(pool)
    .await;
}

/// Delete a team from cache (and its members & shares)
pub async fn delete_team_cache(pool: &SqlitePool, team_id: &str) {
    let _ = sqlx::query("DELETE FROM teams_cache WHERE id = ?").bind(team_id).execute(pool).await;
    let _ = sqlx::query("DELETE FROM team_members_cache WHERE team_id = ?").bind(team_id).execute(pool).await;
    let _ = sqlx::query("DELETE FROM team_shared_folders_cache WHERE team_id = ?").bind(team_id).execute(pool).await;
}

/// Save members of a team to cache (replaces members for that team_id)
pub async fn save_team_members_cache(pool: &SqlitePool, team_id: &str, members: &[RemoteTeamMember]) {
    let _ = sqlx::query("DELETE FROM team_members_cache WHERE team_id = ?").bind(team_id).execute(pool).await;
    for m in members {
        let _ = sqlx::query(
            r#"INSERT OR REPLACE INTO team_members_cache 
            (team_id, user_id, email, display_name, avatar_url, username, phone, role, joined_at) 
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#
        )
        .bind(team_id)
        .bind(&m.user_id)
        .bind(&m.email)
        .bind(&m.display_name)
        .bind(&m.avatar_url)
        .bind(&m.username)
        .bind(&m.phone)
        .bind(&m.role)
        .bind(&m.joined_at)
        .execute(pool)
        .await;
    }
}

/// Save all shared folders to cache
pub async fn save_shared_folders_cache(pool: &SqlitePool, folders: &[RemoteSharedFolder]) {
    let _ = sqlx::query("DELETE FROM team_shared_folders_cache").execute(pool).await;
    for f in folders {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO team_shared_folders_cache (id, team_id, resource_type, folder_path, created_at) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&f.id)
        .bind(&f.team_id)
        .bind(&f.resource_type)
        .bind(&f.folder_path)
        .bind(&f.created_at)
        .execute(pool)
        .await;
    }
}

use std::collections::{HashMap, HashSet};
use eframe::egui;
use sqlx::Row;
use crate::models::enums::{DatabasePool, DatabaseType};

/// Sub-tabs in the User & Role Manager view
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserManagerTab {
    #[default]
    Users,
    CreateUser,
    ObjectGrants,
    SqlPreview,
}

/// User details model
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub username: String,
    pub host: String,
    pub is_superuser: bool,
    pub can_login: bool,
    pub can_create_db: bool,
    pub can_create_role: bool,
    pub is_locked: bool,
    pub password_expired: bool,
    pub valid_until: Option<String>,
    pub member_of: Vec<String>,
    pub attributes: Vec<(String, String)>,
}

/// Role / Group model
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInfo {
    pub role_name: String,
    pub can_login: bool,
    pub is_superuser: bool,
    pub member_count: usize,
    pub members: Vec<String>,
}

/// Object Privilege Matrix Row
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrivilegeEntry {
    pub database: String,
    pub schema: String,
    pub object_name: String,
    pub object_type: String, // "TABLE", "VIEW", "ROUTINE"
    pub has_select: bool,
    pub has_insert: bool,
    pub has_update: bool,
    pub has_delete: bool,
    pub has_execute: bool,
    pub has_all: bool,
    pub grant_option: bool,
    pub is_modified: bool,
}

/// Log of executed SQL query for fetch & diagnostics inspection
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedQueryLog {
    pub step_name: String,
    pub sql: String,
    pub row_count: Option<usize>,
    pub error: Option<String>,
}

/// Form state for creating a new user
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserForm {
    pub username: String,
    pub host: String,
    pub password: String,
    pub confirm_password: String,
    pub can_login: bool,
    pub is_superuser: bool,
    pub can_create_db: bool,
    pub can_create_role: bool,
    pub can_inherit: bool,
    pub selected_roles: HashSet<String>,
    pub show_password: bool,
    pub validation_error: Option<String>,
}

impl Default for NewUserForm {
    fn default() -> Self {
        Self {
            username: String::new(),
            host: "%".to_string(),
            password: String::new(),
            confirm_password: String::new(),
            can_login: true,
            is_superuser: false,
            can_create_db: false,
            can_create_role: false,
            can_inherit: true,
            selected_roles: HashSet::new(),
            show_password: false,
            validation_error: None,
        }
    }
}

/// Form state for changing password
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePasswordForm {
    pub target_user: String,
    pub target_host: String,
    pub new_password: String,
    pub confirm_password: String,
    pub show_password: bool,
    pub validation_error: Option<String>,
}

/// User manager actions communicated to app runner
#[derive(Debug, Clone, PartialEq)]
pub enum UserManagerAction {
    Refresh,
    SelectUser(String, String),
    CreateUser(NewUserForm),
    ChangePassword(ChangePasswordForm),
    DropUser(String, String),
    ApplyPrivilegeChanges {
        grantee: String,
        grantee_host: String,
        sql_statements: Vec<String>,
    },
    OpenChangePasswordModal(String, String),
    CloseChangePasswordModal,
    OpenDropConfirmModal(String, String),
    CloseDropConfirmModal,
    OpenInSqlTab(String),
}

/// Full state of User & Privileges Manager tab
#[derive(Debug, Clone)]
pub struct UserManagerState {
    pub selected_tab: UserManagerTab,
    pub users: Vec<UserInfo>,
    pub roles: Vec<RoleInfo>,
    pub object_grants: Vec<ObjectPrivilegeEntry>,
    pub original_grants: Vec<ObjectPrivilegeEntry>, // To track diffs
    pub all_privileges_map: HashMap<(String, String, String), (HashSet<String>, bool)>,
    pub selected_grantee: Option<String>, // Username or Role currently inspected in matrix
    pub selected_grantee_host: String,
    pub selected_user_index: Option<usize>,
    pub search_text: String,
    pub schema_filter: String,
    pub object_type_filter: String, // "ALL", "TABLE", "VIEW", "ROUTINE"
    pub new_user_form: NewUserForm,
    pub change_password_form: Option<ChangePasswordForm>,
    pub drop_confirm_user: Option<(String, String)>,
    pub generated_sql_log: Vec<String>,
    pub executed_queries: Vec<ExecutedQueryLog>,
    pub show_diagnostics_panel: bool,
    pub is_loading: bool,
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub last_refreshed: Option<std::time::Instant>,
}

impl Default for UserManagerState {
    fn default() -> Self {
        Self {
            selected_tab: UserManagerTab::Users,
            users: Vec::new(),
            roles: Vec::new(),
            object_grants: Vec::new(),
            original_grants: Vec::new(),
            all_privileges_map: HashMap::new(),
            selected_grantee: None,
            selected_grantee_host: "%".to_string(),
            selected_user_index: None,
            search_text: String::new(),
            schema_filter: "ALL".to_string(),
            object_type_filter: "ALL".to_string(),
            new_user_form: NewUserForm::default(),
            change_password_form: None,
            drop_confirm_user: None,
            generated_sql_log: Vec::new(),
            executed_queries: Vec::new(),
            show_diagnostics_panel: false,
            is_loading: false,
            status_message: None,
            last_refreshed: None,
        }
    }
}

impl UserManagerState {
    pub fn sync_grants_for_selected_grantee(&mut self, db_type: Option<&DatabaseType>) {
        let grantee = self.selected_grantee.as_deref().unwrap_or_default();
        let is_mysql = matches!(db_type, Some(DatabaseType::MySQL));

        let target_key = if is_mysql {
            format!("'{}'@'{}'", grantee, self.selected_grantee_host)
        } else {
            grantee.to_string()
        };

        for entry in &mut self.object_grants {
            let priv_entry = self.all_privileges_map
                .get(&(target_key.clone(), entry.schema.clone(), entry.object_name.clone()))
                .or_else(|| {
                    if is_mysql {
                        self.all_privileges_map.get(&(format!("'{}'@'%'", grantee), entry.schema.clone(), entry.object_name.clone()))
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    self.all_privileges_map.iter().find(|((g, s, t), _)| {
                        (g.eq_ignore_ascii_case(&target_key) || (is_mysql && g.starts_with(&format!("'{}'@'", grantee))))
                            && s.eq_ignore_ascii_case(&entry.schema)
                            && t.eq_ignore_ascii_case(&entry.object_name)
                    }).map(|(_, v)| v)
                });

            let (privs, grant_opt) = match priv_entry {
                Some((p, g)) => (p.clone(), *g),
                None => (HashSet::new(), false),
            };

            entry.has_select = privs.contains("SELECT");
            entry.has_insert = privs.contains("INSERT");
            entry.has_update = privs.contains("UPDATE");
            entry.has_delete = privs.contains("DELETE");
            entry.has_execute = privs.contains("EXECUTE");
            entry.has_all = entry.has_select && entry.has_insert && entry.has_update && entry.has_delete;
            entry.grant_option = grant_opt;
            entry.is_modified = false;
        }
        self.original_grants = self.object_grants.clone();
    }
}

/// Payload returned by async fetcher
#[derive(Debug, Clone)]
pub struct UserManagerDataPayload {
    pub users: Vec<UserInfo>,
    pub roles: Vec<RoleInfo>,
    pub object_grants: Vec<ObjectPrivilegeEntry>,
    pub all_privileges_map: HashMap<(String, String, String), (HashSet<String>, bool)>,
    pub executed_queries: Vec<ExecutedQueryLog>,
}

/// Result message passed through mpsc channel to UI thread
#[derive(Debug, Clone)]
pub enum UserManagerResult {
    Data(Result<UserManagerDataPayload, String>),
    CommandExecuted {
        action_name: String,
        sql: String,
        result: Result<String, String>,
    },
}

/// Fetch users, roles, and object grants from the connected database pool
pub async fn fetch_user_manager_data(
    pool: &DatabasePool,
    db_type: &DatabaseType,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<UserManagerDataPayload, String> {
    log::debug!("[USER-MGR] fetch_user_manager_data started for db_type={:?}, database={:?}, schema={:?}", db_type, database_name, schema_name);
    let result = match (db_type, pool) {
        (DatabaseType::PostgreSQL, DatabasePool::PostgreSQL(pg_pool)) => {
            fetch_postgres_user_data(pg_pool).await
        }
        (DatabaseType::MySQL, DatabasePool::MySQL(my_pool)) => {
            fetch_mysql_user_data(my_pool).await
        }
        (DatabaseType::SQLite, DatabasePool::SQLite(sq_pool)) => {
            fetch_sqlite_user_data(sq_pool).await
        }
        _ => Err(format!("User and Role Management is not supported for {:?}", db_type)),
    };
    match &result {
        Ok(payload) => {
            log::debug!("[USER-MGR] fetch_user_manager_data SUCCESS: {} users, {} roles, {} object grants, {} query logs",
                payload.users.len(), payload.roles.len(), payload.object_grants.len(), payload.executed_queries.len());
        }
        Err(err) => {
            log::error!("[USER-MGR] fetch_user_manager_data ERROR: {}", err);
        }
    }
    result
}

/// Execute a user management DDL statement (e.g. CREATE USER, ALTER USER, DROP, GRANT)
pub async fn execute_user_manager_command(
    pool: &DatabasePool,
    query: &str,
) -> Result<(), String> {
    let query_owned = query.to_string();
    match pool {
        DatabasePool::PostgreSQL(pg_pool) => {
            sqlx::query(sqlx::AssertSqlSafe(query_owned))
                .execute(&**pg_pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        DatabasePool::MySQL(my_pool) => {
            for stmt in query_owned.split(';') {
                let trimmed = stmt.trim();
                if !trimmed.is_empty() {
                    sqlx::query(sqlx::AssertSqlSafe(trimmed))
                        .execute(&**my_pool)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        DatabasePool::SQLite(sq_pool) => {
            sqlx::query(sqlx::AssertSqlSafe(query_owned))
                .execute(&**sq_pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        _ => Err("Unsupported database pool for User Manager command".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Query Helpers with Timeouts & Rich Logging
// ---------------------------------------------------------------------------

async fn query_mysql_timeout(
    my_pool: &sqlx::MySqlPool,
    sql: &str,
    timeout_secs: u64,
    step_desc: &str,
) -> Result<Vec<sqlx::mysql::MySqlRow>, String> {
    log::debug!("[USER-MGR-MYSQL] [{}] Starting query (timeout {}s)...", step_desc, timeout_secs);
    
    let fut = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(my_pool);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(Ok(rows)) => {
            log::debug!("[USER-MGR-MYSQL] [{}] SUCCESS: {} rows returned", step_desc, rows.len());
            Ok(rows)
        }
        Ok(Err(e)) => {
            log::warn!("[USER-MGR-MYSQL] [{}] Query Error: {}", step_desc, e);
            Err(e.to_string())
        }
        Err(_) => {
            let err = format!("Query timed out after {}s: {}", timeout_secs, sql.chars().take(60).collect::<String>());
            log::warn!("[USER-MGR-MYSQL] [{}] Timeout Error: {}", step_desc, err);
            Err(err)
        }
    }
}

async fn query_pg_timeout(
    pg_pool: &sqlx::PgPool,
    sql: &str,
    timeout_secs: u64,
    step_desc: &str,
) -> Result<Vec<sqlx::postgres::PgRow>, String> {
    log::debug!("[USER-MGR-PG] [{}] Starting query (timeout {}s)...", step_desc, timeout_secs);
    
    let fut = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(pg_pool);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(Ok(rows)) => {
            log::debug!("[USER-MGR-PG] [{}] SUCCESS: {} rows returned", step_desc, rows.len());
            Ok(rows)
        }
        Ok(Err(e)) => {
            log::warn!("[USER-MGR-PG] [{}] Query Error: {}", step_desc, e);
            Err(e.to_string())
        }
        Err(_) => {
            let err = format!("Query timed out after {}s: {}", timeout_secs, sql.chars().take(60).collect::<String>());
            log::warn!("[USER-MGR-PG] [{}] Timeout Error: {}", step_desc, err);
            Err(err)
        }
    }
}

async fn query_sqlite_timeout(
    sq_pool: &sqlx::SqlitePool,
    sql: &str,
    timeout_secs: u64,
    step_desc: &str,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    log::debug!("[USER-MGR-SQLITE] [{}] Starting query (timeout {}s)...", step_desc, timeout_secs);
    
    let fut = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(sq_pool);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(Ok(rows)) => {
            log::debug!("[USER-MGR-SQLITE] [{}] SUCCESS: {} rows returned", step_desc, rows.len());
            Ok(rows)
        }
        Ok(Err(e)) => {
            log::warn!("[USER-MGR-SQLITE] [{}] Query Error: {}", step_desc, e);
            Err(e.to_string())
        }
        Err(_) => {
            let err = format!("Query timed out after {}s: {}", timeout_secs, sql.chars().take(60).collect::<String>());
            log::warn!("[USER-MGR-SQLITE] [{}] Timeout Error: {}", step_desc, err);
            Err(err)
        }
    }
}

// ---------------------------------------------------------------------------
// Row Field Extraction Helpers (Case-Insensitive & Fallback by Index)
// ---------------------------------------------------------------------------

fn get_col_str_mysql(row: &sqlx::mysql::MySqlRow, col_name: &str, index: usize) -> String {
    let lower = col_name.to_lowercase();
    let upper = col_name.to_uppercase();
    row.try_get::<Option<String>, _>(lower.as_str())
        .or_else(|_| row.try_get::<Option<String>, _>(upper.as_str()))
        .or_else(|_| row.try_get::<Option<String>, _>(col_name))
        .or_else(|_| row.try_get::<Option<String>, _>(index))
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn get_col_str_pg(row: &sqlx::postgres::PgRow, col_name: &str, index: usize) -> String {
    let lower = col_name.to_lowercase();
    let upper = col_name.to_uppercase();
    row.try_get::<Option<String>, _>(lower.as_str())
        .or_else(|_| row.try_get::<Option<String>, _>(upper.as_str()))
        .or_else(|_| row.try_get::<Option<String>, _>(col_name))
        .or_else(|_| row.try_get::<Option<String>, _>(index))
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn get_col_str_sqlite(row: &sqlx::sqlite::SqliteRow, col_name: &str, index: usize) -> String {
    let lower = col_name.to_lowercase();
    let upper = col_name.to_uppercase();
    row.try_get::<Option<String>, _>(lower.as_str())
        .or_else(|_| row.try_get::<Option<String>, _>(upper.as_str()))
        .or_else(|_| row.try_get::<Option<String>, _>(col_name))
        .or_else(|_| row.try_get::<Option<String>, _>(index))
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// PostgreSQL Fetcher
// ---------------------------------------------------------------------------
async fn fetch_postgres_user_data(
    pg_pool: &sqlx::PgPool,
) -> Result<UserManagerDataPayload, String> {
    let mut executed_queries = Vec::new();
    let mut users = Vec::new();
    let mut roles = Vec::new();

    let roles_query = r#"
        SELECT 
            r.rolname AS username,
            r.rolsuper AS is_superuser,
            r.rolinherit AS can_inherit,
            r.rolcreaterole AS can_create_role,
            r.rolcreatedb AS can_create_db,
            r.rolcanlogin AS can_login,
            r.rolreplication AS is_replication,
            r.rolconnlimit AS conn_limit,
            CASE WHEN r.rolvaliduntil IS NULL THEN 'Never' ELSE r.rolvaliduntil::text END AS valid_until
        FROM pg_catalog.pg_roles r
        ORDER BY r.rolname;
    "#;

    let roles_res = query_pg_timeout(pg_pool, roles_query, 4, "Roles (pg_catalog.pg_roles)").await;

    match roles_res {
        Ok(role_rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch PostgreSQL Roles (pg_catalog.pg_roles)".to_string(),
                sql: roles_query.trim().to_string(),
                row_count: Some(role_rows.len()),
                error: None,
            });

            let members_query = r#"
                SELECT 
                    b.rolname AS role_name, 
                    m.rolname AS member_name 
                FROM pg_catalog.pg_auth_members a 
                JOIN pg_catalog.pg_roles b ON (a.roleid = b.oid) 
                JOIN pg_catalog.pg_roles m ON (a.member = m.oid)
                ORDER BY b.rolname, m.rolname;
            "#;
            let member_res = query_pg_timeout(pg_pool, members_query, 4, "Role Members (pg_catalog.pg_auth_members)").await;
            let mut member_to_roles: HashMap<String, Vec<String>> = HashMap::new();
            let mut role_to_members: HashMap<String, Vec<String>> = HashMap::new();

            match member_res {
                Ok(member_rows) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch Role Memberships (pg_catalog.pg_auth_members)".to_string(),
                        sql: members_query.trim().to_string(),
                        row_count: Some(member_rows.len()),
                        error: None,
                    });
                    for row in member_rows {
                        let r_name: String = row.try_get("role_name").unwrap_or_default();
                        let m_name: String = row.try_get("member_name").unwrap_or_default();
                        if !r_name.is_empty() && !m_name.is_empty() {
                            role_to_members.entry(r_name.clone()).or_default().push(m_name.clone());
                            member_to_roles.entry(m_name).or_default().push(r_name);
                        }
                    }
                }
                Err(e) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch Role Memberships (pg_catalog.pg_auth_members)".to_string(),
                        sql: members_query.trim().to_string(),
                        row_count: None,
                        error: Some(e),
                    });
                }
            }

            for row in role_rows {
                let username: String = row.try_get("username").unwrap_or_default();
                let is_superuser: bool = row.try_get("is_superuser").unwrap_or(false);
                let _can_inherit: bool = row.try_get("can_inherit").unwrap_or(true);
                let can_create_role: bool = row.try_get("can_create_role").unwrap_or(false);
                let can_create_db: bool = row.try_get("can_create_db").unwrap_or(false);
                let can_login: bool = row.try_get("can_login").unwrap_or(false);
                let is_replication: bool = row.try_get("is_replication").unwrap_or(false);
                let conn_limit: i32 = row.try_get("conn_limit").unwrap_or(-1);
                let valid_until: String = row.try_get("valid_until").unwrap_or_else(|_| "Never".to_string());

                let member_of = member_to_roles.get(&username).cloned().unwrap_or_default();

                let mut attributes = Vec::new();
                if is_replication {
                    attributes.push(("Replication".to_string(), "Yes".to_string()));
                }
                if conn_limit >= 0 {
                    attributes.push(("Connection Limit".to_string(), conn_limit.to_string()));
                } else {
                    attributes.push(("Connection Limit".to_string(), "Unlimited".to_string()));
                }
                attributes.push(("Valid Until".to_string(), valid_until.clone()));

                users.push(UserInfo {
                    username: username.clone(),
                    host: "localhost".to_string(),
                    is_superuser,
                    can_login,
                    can_create_db,
                    can_create_role,
                    is_locked: false,
                    password_expired: false,
                    valid_until: if valid_until == "Never" { None } else { Some(valid_until) },
                    member_of,
                    attributes,
                });

                if !can_login {
                    let members = role_to_members.get(&username).cloned().unwrap_or_default();
                    roles.push(RoleInfo {
                        role_name: username,
                        can_login: false,
                        is_superuser,
                        member_count: members.len(),
                        members,
                    });
                }
            }
        }
        Err(err_primary) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch PostgreSQL Roles (pg_catalog.pg_roles) - Primary".to_string(),
                sql: roles_query.trim().to_string(),
                row_count: None,
                error: Some(err_primary),
            });

            let pg_user_query = "SELECT usename AS username, usesuper AS is_superuser, usecreatedb AS can_create_db FROM pg_catalog.pg_user ORDER BY usename;";
            let user_res = query_pg_timeout(pg_pool, pg_user_query, 4, "Users Fallback 1 (pg_catalog.pg_user)").await;
            match user_res {
                Ok(user_rows) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch PostgreSQL Users (pg_catalog.pg_user) - Fallback 1".to_string(),
                        sql: pg_user_query.to_string(),
                        row_count: Some(user_rows.len()),
                        error: None,
                    });
                    for row in user_rows {
                        let uname: String = row.try_get("username").unwrap_or_default();
                        let is_sup: bool = row.try_get("is_superuser").unwrap_or(false);
                        let can_cdb: bool = row.try_get("can_create_db").unwrap_or(false);
                        users.push(UserInfo {
                            username: uname,
                            host: "localhost".to_string(),
                            is_superuser: is_sup,
                            can_login: true,
                            can_create_db: can_cdb,
                            can_create_role: is_sup,
                            is_locked: false,
                            password_expired: false,
                            valid_until: None,
                            member_of: Vec::new(),
                            attributes: vec![("Source".to_string(), "pg_catalog.pg_user".to_string())],
                        });
                    }
                }
                Err(err_fb1) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch PostgreSQL Users (pg_catalog.pg_user) - Fallback 1".to_string(),
                        sql: pg_user_query.to_string(),
                        row_count: None,
                        error: Some(err_fb1),
                    });

                    let cur_user_query = "SELECT current_user AS username, session_user AS session_user;";
                    let cur_res = query_pg_timeout(pg_pool, cur_user_query, 4, "Current User Fallback 2").await;
                    match cur_res {
                        Ok(rows) => {
                            executed_queries.push(ExecutedQueryLog {
                                step_name: "Fetch Current PostgreSQL User - Fallback 2".to_string(),
                                sql: cur_user_query.to_string(),
                                row_count: Some(rows.len()),
                                error: None,
                            });
                            for r in rows {
                                let uname: String = r.try_get("username").unwrap_or_else(|_| "current_user".to_string());
                                users.push(UserInfo {
                                    username: uname,
                                    host: "localhost".to_string(),
                                    is_superuser: false,
                                    can_login: true,
                                    can_create_db: false,
                                    can_create_role: false,
                                    is_locked: false,
                                    password_expired: false,
                                    valid_until: None,
                                    member_of: Vec::new(),
                                    attributes: vec![("Source".to_string(), "current_user()".to_string())],
                                });
                            }
                        }
                        Err(err_fb2) => {
                            executed_queries.push(ExecutedQueryLog {
                                step_name: "Fetch Current PostgreSQL User - Fallback 2".to_string(),
                                sql: cur_user_query.to_string(),
                                row_count: None,
                                error: Some(err_fb2),
                            });
                        }
                    }
                }
            }
        }
    }

    let tables_query = r#"
        SELECT 
            table_catalog,
            table_schema, 
            table_name, 
            table_type 
        FROM information_schema.tables 
        WHERE table_schema NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
        ORDER BY table_schema, table_name;
    "#;
    let table_rows = match query_pg_timeout(pg_pool, tables_query, 4, "Tables & Views").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch Tables & Views (information_schema.tables)".to_string(),
                sql: tables_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch Tables & Views (information_schema.tables)".to_string(),
                sql: tables_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let privs_query = r#"
        SELECT 
            grantee, 
            table_schema, 
            table_name, 
            privilege_type, 
            is_grantable 
        FROM information_schema.table_privileges 
        WHERE table_schema NOT IN ('pg_catalog', 'information_schema', 'pg_toast');
    "#;
    let priv_rows = match query_pg_timeout(pg_pool, privs_query, 4, "Table Privileges").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch Table Privileges (information_schema.table_privileges)".to_string(),
                sql: privs_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch Table Privileges (information_schema.table_privileges)".to_string(),
                sql: privs_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let mut priv_map: HashMap<(String, String, String), (HashSet<String>, bool)> = HashMap::new();
    for row in priv_rows {
        let grantee = get_col_str_pg(&row, "grantee", 0);
        let schema = get_col_str_pg(&row, "table_schema", 1);
        let table = get_col_str_pg(&row, "table_name", 2);
        let priv_type = get_col_str_pg(&row, "privilege_type", 3);
        let is_grantable_str = get_col_str_pg(&row, "is_grantable", 4);
        let is_grantable = is_grantable_str.eq_ignore_ascii_case("YES");

        let entry = priv_map.entry((grantee, schema, table)).or_insert_with(|| (HashSet::new(), false));
        entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            entry.1 = true;
        }
    }

    for user in &users {
        if user.is_superuser {
            for row in &table_rows {
                let schema = get_col_str_pg(row, "table_schema", 1);
                let table_name = get_col_str_pg(row, "table_name", 2);
                let mut super_privs = HashSet::new();
                super_privs.insert("SELECT".to_string());
                super_privs.insert("INSERT".to_string());
                super_privs.insert("UPDATE".to_string());
                super_privs.insert("DELETE".to_string());
                super_privs.insert("EXECUTE".to_string());
                super_privs.insert("ALL".to_string());
                priv_map.insert((user.username.clone(), schema, table_name), (super_privs, true));
            }
        }
    }

    let default_grantee = users.first().map(|u| u.username.as_str()).unwrap_or("public");
    let mut object_grants = Vec::new();

    for row in table_rows {
        let mut db = get_col_str_pg(&row, "table_catalog", 0);
        if db.is_empty() {
            db = "postgres".to_string();
        }
        let schema = get_col_str_pg(&row, "table_schema", 1);
        let table_name = get_col_str_pg(&row, "table_name", 2);
        let mut ttype = get_col_str_pg(&row, "table_type", 3);
        if ttype.is_empty() {
            ttype = "BASE TABLE".to_string();
        }

        let (privs, grant_opt) = priv_map
            .get(&(default_grantee.to_string(), schema.clone(), table_name.clone()))
            .cloned()
            .unwrap_or_default();

        let has_select = privs.contains("SELECT");
        let has_insert = privs.contains("INSERT");
        let has_update = privs.contains("UPDATE");
        let has_delete = privs.contains("DELETE");
        let has_execute = privs.contains("EXECUTE");
        let has_all = has_select && has_insert && has_update && has_delete;

        object_grants.push(ObjectPrivilegeEntry {
            database: db,
            schema,
            object_name: table_name,
            object_type: if ttype.contains("VIEW") { "VIEW".to_string() } else { "TABLE".to_string() },
            has_select,
            has_insert,
            has_update,
            has_delete,
            has_execute,
            has_all,
            grant_option: grant_opt,
            is_modified: false,
        });
    }

    Ok(UserManagerDataPayload {
        users,
        roles,
        object_grants,
        all_privileges_map: priv_map,
        executed_queries,
    })
}

// ---------------------------------------------------------------------------
// MySQL Fetcher
// ---------------------------------------------------------------------------
async fn fetch_mysql_user_data(
    my_pool: &sqlx::MySqlPool,
) -> Result<UserManagerDataPayload, String> {
    let mut executed_queries = Vec::new();
    let mut users = Vec::new();
    let roles = Vec::new();

    let users_query_1 = r#"
        SELECT 
            User, 
            Host, 
            plugin, 
            account_locked, 
            password_expired 
        FROM mysql.user 
        ORDER BY User, Host;
    "#;

    let res_1 = query_mysql_timeout(my_pool, users_query_1, 4, "Users Attempt 1 (mysql.user full)").await;
    match res_1 {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Users (mysql.user full) - Attempt 1".to_string(),
                sql: users_query_1.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            for row in rows {
                let user: String = row.try_get("User").unwrap_or_default();
                let host: String = row.try_get("Host").unwrap_or_else(|_| "%".to_string());
                let plugin: String = row.try_get("plugin").unwrap_or_default();
                let locked_str: String = row.try_get("account_locked").unwrap_or_else(|_| "N".to_string());
                let exp_str: String = row.try_get("password_expired").unwrap_or_else(|_| "N".to_string());

                let is_locked = locked_str.eq_ignore_ascii_case("Y");
                let password_expired = exp_str.eq_ignore_ascii_case("Y");
                let is_superuser = user == "root";

                let mut attributes = Vec::new();
                if !plugin.is_empty() {
                    attributes.push(("Auth Plugin".to_string(), plugin));
                }
                attributes.push(("Host Scope".to_string(), host.clone()));

                users.push(UserInfo {
                    username: user,
                    host,
                    is_superuser,
                    can_login: !is_locked,
                    can_create_db: is_superuser,
                    can_create_role: is_superuser,
                    is_locked,
                    password_expired,
                    valid_until: None,
                    member_of: Vec::new(),
                    attributes,
                });
            }
        }
        Err(err_1) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Users (mysql.user full) - Attempt 1".to_string(),
                sql: users_query_1.trim().to_string(),
                row_count: None,
                error: Some(err_1),
            });

            let users_query_2 = "SELECT DISTINCT User, Host FROM mysql.user ORDER BY User, Host;";
            let res_2 = query_mysql_timeout(my_pool, users_query_2, 4, "Users Attempt 2 (mysql.user minimal)").await;
            match res_2 {
                Ok(rows) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch MySQL Users (mysql.user minimal) - Attempt 2".to_string(),
                        sql: users_query_2.to_string(),
                        row_count: Some(rows.len()),
                        error: None,
                    });
                    for row in rows {
                        let user: String = row.try_get("User").unwrap_or_default();
                        let host: String = row.try_get("Host").unwrap_or_else(|_| "%".to_string());
                        users.push(UserInfo {
                            username: user.clone(),
                            host: host.clone(),
                            is_superuser: user == "root",
                            can_login: true,
                            can_create_db: user == "root",
                            can_create_role: user == "root",
                            is_locked: false,
                            password_expired: false,
                            valid_until: None,
                            member_of: Vec::new(),
                            attributes: vec![("Host Scope".to_string(), host)],
                        });
                    }
                }
                Err(err_2) => {
                    executed_queries.push(ExecutedQueryLog {
                        step_name: "Fetch MySQL Users (mysql.user minimal) - Attempt 2".to_string(),
                        sql: users_query_2.to_string(),
                        row_count: None,
                        error: Some(err_2),
                    });

                    let users_query_3 = "SELECT DISTINCT GRANTEE FROM information_schema.user_privileges ORDER BY GRANTEE;";
                    let res_3 = query_mysql_timeout(my_pool, users_query_3, 4, "Users Attempt 3 (user_privileges)").await;
                    match res_3 {
                        Ok(rows) if !rows.is_empty() => {
                            executed_queries.push(ExecutedQueryLog {
                                step_name: "Fetch MySQL Grantees (information_schema.user_privileges) - Attempt 3".to_string(),
                                sql: users_query_3.to_string(),
                                row_count: Some(rows.len()),
                                error: None,
                            });
                            for row in rows {
                                let grantee: String = row.try_get("GRANTEE").unwrap_or_default();
                                let clean = grantee.replace('\'', "");
                                let parts: Vec<&str> = clean.split('@').collect();
                                let user = parts.get(0).copied().unwrap_or("unknown").to_string();
                                let host = parts.get(1).copied().unwrap_or("%").to_string();
                                users.push(UserInfo {
                                    username: user.clone(),
                                    host: host.clone(),
                                    is_superuser: user == "root",
                                    can_login: true,
                                    can_create_db: user == "root",
                                    can_create_role: user == "root",
                                    is_locked: false,
                                    password_expired: false,
                                    valid_until: None,
                                    member_of: Vec::new(),
                                    attributes: vec![("Grantee Spec".to_string(), grantee)],
                                });
                            }
                        }
                        _ => {
                            let users_query_4 = "SELECT CURRENT_USER() AS cur_user, USER() AS session_user;";
                            let res_4 = query_mysql_timeout(my_pool, users_query_4, 4, "Users Attempt 4 (CURRENT_USER)").await;
                            match res_4 {
                                Ok(rows) => {
                                    executed_queries.push(ExecutedQueryLog {
                                        step_name: "Fetch Current MySQL User (CURRENT_USER()) - Attempt 4".to_string(),
                                        sql: users_query_4.to_string(),
                                        row_count: Some(rows.len()),
                                        error: None,
                                    });
                                    for row in rows {
                                        let cur: String = row.try_get("cur_user").unwrap_or_default();
                                        let clean = cur.replace('\'', "");
                                        let parts: Vec<&str> = clean.split('@').collect();
                                        let user = parts.get(0).copied().unwrap_or("current_user").to_string();
                                        let host = parts.get(1).copied().unwrap_or("%").to_string();
                                        users.push(UserInfo {
                                            username: user,
                                            host,
                                            is_superuser: false,
                                            can_login: true,
                                            can_create_db: false,
                                            can_create_role: false,
                                            is_locked: false,
                                            password_expired: false,
                                            valid_until: None,
                                            member_of: Vec::new(),
                                            attributes: vec![("Source".to_string(), "CURRENT_USER()".to_string())],
                                        });
                                    }
                                }
                                Err(err_4) => {
                                    executed_queries.push(ExecutedQueryLog {
                                        step_name: "Fetch Current MySQL User (CURRENT_USER()) - Attempt 4".to_string(),
                                        sql: users_query_4.to_string(),
                                        row_count: None,
                                        error: Some(err_4),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let tables_query = r#"
        SELECT 
            table_schema, 
            table_name, 
            table_type 
        FROM information_schema.tables 
        WHERE table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
        ORDER BY table_schema, table_name;
    "#;
    let table_rows = match query_mysql_timeout(my_pool, tables_query, 4, "Tables List").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Tables (information_schema.tables)".to_string(),
                sql: tables_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Tables (information_schema.tables)".to_string(),
                sql: tables_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    // 1. Global User Privileges
    let user_privs_query = r#"
        SELECT 
            grantee, 
            privilege_type, 
            is_grantable 
        FROM information_schema.user_privileges;
    "#;
    let user_priv_rows = match query_mysql_timeout(my_pool, user_privs_query, 4, "Global User Privileges").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Global Privileges (information_schema.user_privileges)".to_string(),
                sql: user_privs_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Global Privileges (information_schema.user_privileges)".to_string(),
                sql: user_privs_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let mut global_priv_map: HashMap<String, (HashSet<String>, bool)> = HashMap::new();
    for row in user_priv_rows {
        let grantee = get_col_str_mysql(&row, "grantee", 0);
        let priv_type = get_col_str_mysql(&row, "privilege_type", 1);
        let is_grantable_str = get_col_str_mysql(&row, "is_grantable", 2);
        let is_grantable = is_grantable_str.eq_ignore_ascii_case("YES");

        let entry = global_priv_map.entry(grantee.clone()).or_insert_with(|| (HashSet::new(), false));
        entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            entry.1 = true;
        }

        let clean = grantee.replace('\'', "");
        let clean_entry = global_priv_map.entry(clean).or_insert_with(|| (HashSet::new(), false));
        clean_entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            clean_entry.1 = true;
        }
    }

    // 2. Schema / Database Level Privileges
    let schema_privs_query = r#"
        SELECT 
            grantee, 
            table_schema, 
            privilege_type, 
            is_grantable 
        FROM information_schema.schema_privileges 
        WHERE table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys');
    "#;
    let schema_priv_rows = match query_mysql_timeout(my_pool, schema_privs_query, 4, "Schema Privileges").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Schema Privileges (information_schema.schema_privileges)".to_string(),
                sql: schema_privs_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Schema Privileges (information_schema.schema_privileges)".to_string(),
                sql: schema_privs_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let mut schema_priv_map: HashMap<(String, String), (HashSet<String>, bool)> = HashMap::new();
    for row in schema_priv_rows {
        let grantee = get_col_str_mysql(&row, "grantee", 0);
        let schema = get_col_str_mysql(&row, "table_schema", 1);
        let priv_type = get_col_str_mysql(&row, "privilege_type", 2);
        let is_grantable_str = get_col_str_mysql(&row, "is_grantable", 3);
        let is_grantable = is_grantable_str.eq_ignore_ascii_case("YES");

        let entry = schema_priv_map.entry((grantee.clone(), schema.clone())).or_insert_with(|| (HashSet::new(), false));
        entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            entry.1 = true;
        }

        let clean = grantee.replace('\'', "");
        let clean_entry = schema_priv_map.entry((clean, schema)).or_insert_with(|| (HashSet::new(), false));
        clean_entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            clean_entry.1 = true;
        }
    }

    // 3. Table-Level Privileges
    let privs_query = r#"
        SELECT 
            grantee, 
            table_schema, 
            table_name, 
            privilege_type, 
            is_grantable 
        FROM information_schema.table_privileges 
        WHERE table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys');
    "#;
    let priv_rows = match query_mysql_timeout(my_pool, privs_query, 4, "Table Privileges").await {
        Ok(rows) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Table Privileges (information_schema.table_privileges)".to_string(),
                sql: privs_query.trim().to_string(),
                row_count: Some(rows.len()),
                error: None,
            });
            rows
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch MySQL Table Privileges (information_schema.table_privileges)".to_string(),
                sql: privs_query.trim().to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let mut table_priv_map: HashMap<(String, String, String), (HashSet<String>, bool)> = HashMap::new();
    for row in priv_rows {
        let grantee = get_col_str_mysql(&row, "grantee", 0);
        let schema = get_col_str_mysql(&row, "table_schema", 1);
        let table = get_col_str_mysql(&row, "table_name", 2);
        let priv_type = get_col_str_mysql(&row, "privilege_type", 3);
        let is_grantable_str = get_col_str_mysql(&row, "is_grantable", 4);
        let is_grantable = is_grantable_str.eq_ignore_ascii_case("YES");

        let entry = table_priv_map.entry((grantee.clone(), schema.clone(), table.clone())).or_insert_with(|| (HashSet::new(), false));
        entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            entry.1 = true;
        }

        let clean = grantee.replace('\'', "");
        let clean_entry = table_priv_map.entry((clean, schema, table)).or_insert_with(|| (HashSet::new(), false));
        clean_entry.0.insert(priv_type.to_uppercase());
        if is_grantable {
            clean_entry.1 = true;
        }
    }

    // Collect all table objects
    let mut table_objects = Vec::new();
    for row in table_rows {
        let schema = get_col_str_mysql(&row, "table_schema", 0);
        let table_name = get_col_str_mysql(&row, "table_name", 1);
        let mut ttype = get_col_str_mysql(&row, "table_type", 2);
        if ttype.is_empty() {
            ttype = "BASE TABLE".to_string();
        }
        table_objects.push((schema, table_name, ttype));
    }

    // Compute effective permissions for each user
    let mut all_privileges_map: HashMap<(String, String, String), (HashSet<String>, bool)> = HashMap::new();
    for user in &users {
        let is_root = user.username.eq_ignore_ascii_case("root") || user.is_superuser;
        let user_keys = [
            format!("'{}'@'{}'", user.username, user.host),
            format!("'{}'@'%'", user.username),
            format!("{}@{}", user.username, user.host),
            user.username.clone(),
        ];

        for (schema, table_name, _) in &table_objects {
            let mut privs = HashSet::new();
            let mut grant_opt = false;

            if is_root {
                privs.insert("SELECT".to_string());
                privs.insert("INSERT".to_string());
                privs.insert("UPDATE".to_string());
                privs.insert("DELETE".to_string());
                privs.insert("EXECUTE".to_string());
                privs.insert("ALL".to_string());
                grant_opt = true;
            } else {
                for key in &user_keys {
                    if let Some((p, g)) = global_priv_map.get(key) {
                        for item in p { privs.insert(item.clone()); }
                        if *g { grant_opt = true; }
                    }
                    if let Some((p, g)) = schema_priv_map.get(&(key.clone(), schema.clone())) {
                        for item in p { privs.insert(item.clone()); }
                        if *g { grant_opt = true; }
                    }
                    if let Some((p, g)) = table_priv_map.get(&(key.clone(), schema.clone(), table_name.clone())) {
                        for item in p { privs.insert(item.clone()); }
                        if *g { grant_opt = true; }
                    }
                }

                if privs.contains("ALL") || privs.contains("ALL PRIVILEGES") {
                    privs.insert("SELECT".to_string());
                    privs.insert("INSERT".to_string());
                    privs.insert("UPDATE".to_string());
                    privs.insert("DELETE".to_string());
                    privs.insert("EXECUTE".to_string());
                }
            }

            all_privileges_map.insert((format!("'{}'@'{}'", user.username, user.host), schema.clone(), table_name.clone()), (privs.clone(), grant_opt));
            all_privileges_map.insert((user.username.clone(), schema.clone(), table_name.clone()), (privs, grant_opt));
        }
    }

    let default_grantee_user = users.first();
    let mut object_grants = Vec::new();

    for (schema, table_name, ttype) in table_objects {
        let (privs, grant_opt) = if let Some(u) = default_grantee_user {
            all_privileges_map
                .get(&(format!("'{}'@'{}'", u.username, u.host), schema.clone(), table_name.clone()))
                .cloned()
                .unwrap_or_default()
        } else {
            (HashSet::new(), false)
        };

        let has_select = privs.contains("SELECT");
        let has_insert = privs.contains("INSERT");
        let has_update = privs.contains("UPDATE");
        let has_delete = privs.contains("DELETE");
        let has_execute = privs.contains("EXECUTE");
        let has_all = has_select && has_insert && has_update && has_delete;

        object_grants.push(ObjectPrivilegeEntry {
            database: schema.clone(),
            schema: schema.clone(),
            object_name: table_name,
            object_type: if ttype.contains("VIEW") { "VIEW".to_string() } else { "TABLE".to_string() },
            has_select,
            has_insert,
            has_update,
            has_delete,
            has_execute,
            has_all,
            grant_option: grant_opt,
            is_modified: false,
        });
    }

    Ok(UserManagerDataPayload {
        users,
        roles,
        object_grants,
        all_privileges_map,
        executed_queries,
    })
}

// ---------------------------------------------------------------------------
// SQLite Fetcher
// ---------------------------------------------------------------------------
async fn fetch_sqlite_user_data(
    sq_pool: &sqlx::SqlitePool,
) -> Result<UserManagerDataPayload, String> {
    let mut executed_queries = Vec::new();
    let mut users = Vec::new();
    users.push(UserInfo {
        username: "sqlite_master".to_string(),
        host: "embedded (local file)".to_string(),
        is_superuser: true,
        can_login: true,
        can_create_db: true,
        can_create_role: false,
        is_locked: false,
        password_expired: false,
        valid_until: None,
        member_of: vec!["Database Owner".to_string()],
        attributes: vec![
            ("Storage Mode".to_string(), "Single-File / Serverless".to_string()),
            ("Security Model".to_string(), "OS File Permissions".to_string()),
        ],
    });

    let tables_query = "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name;";
    let rows = match query_sqlite_timeout(sq_pool, tables_query, 4, "SQLite Objects").await {
        Ok(r) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch SQLite Objects (sqlite_master)".to_string(),
                sql: tables_query.to_string(),
                row_count: Some(r.len()),
                error: None,
            });
            r
        }
        Err(e) => {
            executed_queries.push(ExecutedQueryLog {
                step_name: "Fetch SQLite Objects (sqlite_master)".to_string(),
                sql: tables_query.to_string(),
                row_count: None,
                error: Some(e),
            });
            Vec::new()
        }
    };

    let mut object_grants = Vec::new();
    for r in rows {
        let name = get_col_str_sqlite(&r, "name", 0);
        let mut otype = get_col_str_sqlite(&r, "type", 1);
        if otype.is_empty() {
            otype = "table".to_string();
        }
        object_grants.push(ObjectPrivilegeEntry {
            database: "main".to_string(),
            schema: "main".to_string(),
            object_name: name,
            object_type: otype.to_uppercase(),
            has_select: true,
            has_insert: true,
            has_update: true,
            has_delete: true,
            has_execute: false,
            has_all: true,
            grant_option: true,
            is_modified: false,
        });
    }

    Ok(UserManagerDataPayload {
        users,
        roles: Vec::new(),
        object_grants,
        all_privileges_map: HashMap::new(),
        executed_queries,
    })
}

// ---------------------------------------------------------------------------
// SQL Statement Generators
// ---------------------------------------------------------------------------

pub fn generate_create_user_sql(form: &NewUserForm, db_type: &DatabaseType) -> String {
    let mut stmts = Vec::new();
    match db_type {
        DatabaseType::PostgreSQL => {
            let mut opts = Vec::new();
            if !form.password.is_empty() {
                opts.push(format!("PASSWORD '{}'", form.password.replace('\'', "''")));
            }
            if form.can_login { opts.push("LOGIN".to_string()); } else { opts.push("NOLOGIN".to_string()); }
            if form.is_superuser { opts.push("SUPERUSER".to_string()); } else { opts.push("NOSUPERUSER".to_string()); }
            if form.can_create_db { opts.push("CREATEDB".to_string()); } else { opts.push("NOCREATEDB".to_string()); }
            if form.can_create_role { opts.push("CREATEROLE".to_string()); } else { opts.push("NOCREATEROLE".to_string()); }
            if form.can_inherit { opts.push("INHERIT".to_string()); } else { opts.push("NOINHERIT".to_string()); }

            let create_stmt = format!("CREATE ROLE \"{}\" WITH {};", form.username.replace('"', "\"\""), opts.join(" "));
            stmts.push(create_stmt);

            for role in &form.selected_roles {
                stmts.push(format!("GRANT \"{}\" TO \"{}\";", role.replace('"', "\"\""), form.username.replace('"', "\"\"")));
            }
        }
        DatabaseType::MySQL => {
            let host_part = if form.host.is_empty() { "%" } else { &form.host };
            let auth_clause = if !form.password.is_empty() {
                format!(" IDENTIFIED BY '{}'", form.password.replace('\'', "\\'"))
            } else {
                String::new()
            };
            let create_stmt = format!("CREATE USER '{}'@'{}'{};", form.username.replace('\'', "\\'"), host_part, auth_clause);
            stmts.push(create_stmt);

            if form.is_superuser {
                stmts.push(format!("GRANT ALL PRIVILEGES ON *.* TO '{}'@'{}' WITH GRANT OPTION;", form.username.replace('\'', "\\'"), host_part));
            }
            stmts.push("FLUSH PRIVILEGES;".to_string());
        }
        DatabaseType::MsSQL => {
            let login_stmt = if !form.password.is_empty() {
                format!("CREATE LOGIN [{}] WITH PASSWORD = '{}';", form.username.replace(']', "]]"), form.password.replace('\'', "''"))
            } else {
                format!("CREATE LOGIN [{}] WITHOUT LOGIN;", form.username.replace(']', "]]"))
            };
            stmts.push(login_stmt);
            stmts.push(format!("CREATE USER [{}] FOR LOGIN [{}];", form.username.replace(']', "]]"), form.username.replace(']', "]]")));
            if form.is_superuser {
                stmts.push(format!("ALTER SERVER ROLE sysadmin ADD MEMBER [{}];", form.username.replace(']', "]]")));
            }
        }
        DatabaseType::SQLite => {
            stmts.push("-- SQLite uses file-system security. No DDL required.".to_string());
        }
        _ => {
            stmts.push(format!("-- User creation not supported for {:?}", db_type));
        }
    }
    stmts.join("\n")
}

pub fn generate_alter_password_sql(username: &str, host: &str, new_pass: &str, db_type: &DatabaseType) -> String {
    match db_type {
        DatabaseType::PostgreSQL => {
            format!("ALTER ROLE \"{}\" WITH PASSWORD '{}';", username.replace('"', "\"\""), new_pass.replace('\'', "''"))
        }
        DatabaseType::MySQL => {
            let host_part = if host.is_empty() { "%" } else { host };
            format!("ALTER USER '{}'@'{}' IDENTIFIED BY '{}';\nFLUSH PRIVILEGES;", username.replace('\'', "\\'"), host_part, new_pass.replace('\'', "\\'"))
        }
        DatabaseType::MsSQL => {
            format!("ALTER LOGIN [{}] WITH PASSWORD = '{}';", username.replace(']', "]]"), new_pass.replace('\'', "''"))
        }
        _ => "-- Password change not supported for this database".to_string(),
    }
}

pub fn generate_drop_user_sql(username: &str, host: &str, db_type: &DatabaseType) -> String {
    match db_type {
        DatabaseType::PostgreSQL => {
            format!("DROP ROLE \"{}\";", username.replace('"', "\"\""))
        }
        DatabaseType::MySQL => {
            let host_part = if host.is_empty() { "%" } else { host };
            format!("DROP USER '{}'@'{}';", username.replace('\'', "\\'"), host_part)
        }
        DatabaseType::MsSQL => {
            format!("DROP USER IF EXISTS [{}];\nDROP LOGIN [{}];", username.replace(']', "]]"), username.replace(']', "]]"))
        }
        _ => "-- Drop user not supported for this database".to_string(),
    }
}

pub fn generate_object_privilege_diff_sql(
    grantee: &str,
    grantee_host: &str,
    original: &ObjectPrivilegeEntry,
    updated: &ObjectPrivilegeEntry,
    db_type: &DatabaseType,
) -> Vec<String> {
    let mut statements = Vec::new();
    let privs = [
        ("SELECT", original.has_select, updated.has_select),
        ("INSERT", original.has_insert, updated.has_insert),
        ("UPDATE", original.has_update, updated.has_update),
        ("DELETE", original.has_delete, updated.has_delete),
        ("EXECUTE", original.has_execute, updated.has_execute),
    ];

    for (p_name, orig_val, new_val) in privs {
        if orig_val != new_val {
            match db_type {
                DatabaseType::PostgreSQL => {
                    let target_obj = format!("\"{}\".\"{}\"", original.schema.replace('"', "\"\""), original.object_name.replace('"', "\"\""));
                    let kind_prefix = if original.object_type == "ROUTINE" || original.object_type == "FUNCTION" { "FUNCTION " } else { "TABLE " };
                    if new_val {
                        let grant_opt = if updated.grant_option { " WITH GRANT OPTION" } else { "" };
                        statements.push(format!("GRANT {} ON {}{} TO \"{}\"{};", p_name, kind_prefix, target_obj, grantee.replace('"', "\"\""), grant_opt));
                    } else {
                        statements.push(format!("REVOKE {} ON {}{} FROM \"{}\";", p_name, kind_prefix, target_obj, grantee.replace('"', "\"\"")));
                    }
                }
                DatabaseType::MySQL => {
                    let host_part = if grantee_host.is_empty() { "%" } else { grantee_host };
                    let target_obj = format!("`{}`.`{}`", original.schema.replace('`', "``"), original.object_name.replace('`', "``"));
                    if new_val {
                        let grant_opt = if updated.grant_option { " WITH GRANT OPTION" } else { "" };
                        statements.push(format!("GRANT {} ON {} TO '{}'@'{}'{};", p_name, target_obj, grantee.replace('\'', "\\'"), host_part, grant_opt));
                    } else {
                        statements.push(format!("REVOKE {} ON {} FROM '{}'@'{}';", p_name, target_obj, grantee.replace('\'', "\\'"), host_part));
                    }
                }
                _ => {}
            }
        }
    }
    if db_type == &DatabaseType::MySQL && !statements.is_empty() {
        statements.push("FLUSH PRIVILEGES;".to_string());
    }
    statements
}

// ---------------------------------------------------------------------------
// GUI Rendering
// ---------------------------------------------------------------------------

pub fn render_user_manager(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    db_type: Option<&DatabaseType>,
    conn_name: &str,
    out_action: &mut Option<UserManagerAction>,
) {
    let ctx = ui.ctx().clone();

    render_modals(&ctx, state, db_type, out_action);
    render_header_bar(ui, state, db_type, conn_name, out_action);
    ui.add_space(4.0);

    if let Some((msg, is_error)) = &state.status_message {
        render_status_banner(ui, msg, *is_error);
        ui.add_space(4.0);
    }

    egui::Frame::group(ui.style())
        .fill(ui.visuals().window_fill())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| match state.selected_tab {
            UserManagerTab::Users => render_users_and_roles_tab(ui, state, db_type, out_action),
            UserManagerTab::CreateUser => render_create_user_tab(ui, state, db_type, out_action),
            UserManagerTab::ObjectGrants => render_object_grants_matrix_tab(ui, state, db_type, out_action),
            UserManagerTab::SqlPreview => render_sql_preview_tab(ui, state, out_action),
        });

    if state.show_diagnostics_panel || (state.users.is_empty() && !state.is_loading && state.selected_tab != UserManagerTab::SqlPreview) {
        ui.add_space(6.0);
        render_diagnostics_card(ui, state, out_action);
    }
}

fn render_header_bar(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    _db_type: Option<&DatabaseType>,
    conn_name: &str,
    out_action: &mut Option<UserManagerAction>,
) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} User & Privileges Manager", egui_icons::icons::ICON_GROUP.codepoint))
                        .strong()
                        .size(15.0)
                        .color(ui.visuals().strong_text_color()),
                );

                ui.add_space(8.0);
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!("🔗 {}", conn_name))
                                .size(11.0)
                                .color(ui.visuals().text_color()),
                        );
                    });

                ui.add_space(16.0);

                let user_count = state.users.len();
                let role_count = state.roles.len();
                let users_label = format!("👥 Users & Roles ({}+{})", user_count, role_count);
                if ui.selectable_label(state.selected_tab == UserManagerTab::Users, users_label).clicked() {
                    state.selected_tab = UserManagerTab::Users;
                }

                if ui.selectable_label(state.selected_tab == UserManagerTab::CreateUser, "➕ New User").clicked() {
                    state.selected_tab = UserManagerTab::CreateUser;
                }

                let modified_count = state.object_grants.iter().filter(|g| g.is_modified).count();
                let grants_label = if modified_count > 0 {
                    format!("🛡️ Object Grants (● {})", modified_count)
                } else {
                    "🛡️ Object Grants Matrix".to_string()
                };
                if ui.selectable_label(state.selected_tab == UserManagerTab::ObjectGrants, grants_label).clicked() {
                    state.selected_tab = UserManagerTab::ObjectGrants;
                }

                let diag_errors = state.executed_queries.iter().filter(|q| q.error.is_some()).count();
                let sql_tab_label = if diag_errors > 0 {
                    format!("📜 SQL & Diagnostics (⚠️ {})", diag_errors)
                } else {
                    "📜 SQL & Diagnostics".to_string()
                };
                if ui.selectable_label(state.selected_tab == UserManagerTab::SqlPreview, sql_tab_label).clicked() {
                    state.selected_tab = UserManagerTab::SqlPreview;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let refresh_btn = egui::Button::new(
                        egui::RichText::new(format!("{} Refresh", egui_icons::icons::ICON_REFRESH.codepoint))
                            .size(12.0),
                    );
                    if ui.add_enabled(!state.is_loading, refresh_btn).clicked() {
                        *out_action = Some(UserManagerAction::Refresh);
                    }

                    if state.is_loading {
                        ui.spinner();
                        ui.label(egui::RichText::new("Loading...").italics().size(11.0));
                    } else if let Some(last) = state.last_refreshed {
                        ui.label(
                            egui::RichText::new(format!("Updated {:.0}s ago", last.elapsed().as_secs_f32()))
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    }

                    let diag_btn_text = if state.show_diagnostics_panel {
                        "🔍 Hide SQL Queries"
                    } else {
                        "🔍 Show SQL Queries"
                    };
                    if ui.small_button(diag_btn_text).clicked() {
                        state.show_diagnostics_panel = !state.show_diagnostics_panel;
                    }

                    if state.selected_tab == UserManagerTab::Users || state.selected_tab == UserManagerTab::ObjectGrants {
                        ui.add_space(8.0);
                        let search_edit = egui::TextEdit::singleline(&mut state.search_text)
                            .hint_text("🔍 Search users, tables, roles...")
                            .desired_width(180.0);
                        ui.add(search_edit);
                    }
                });
            });
        });
}

fn render_status_banner(ui: &mut egui::Ui, message: &str, is_error: bool) {
    let (bg_color, text_color, icon) = if is_error {
        (
            egui::Color32::from_rgb(80, 20, 20),
            egui::Color32::from_rgb(255, 180, 180),
            "⚠️",
        )
    } else {
        (
            egui::Color32::from_rgb(20, 70, 30),
            egui::Color32::from_rgb(180, 255, 190),
            "✅",
        )
    };

    egui::Frame::group(ui.style())
        .fill(bg_color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} {}", icon, message)).color(text_color).size(12.0));
            });
        });
}

fn render_users_and_roles_tab(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    db_type: Option<&DatabaseType>,
    out_action: &mut Option<UserManagerAction>,
) {
    let filter_text = state.search_text.to_lowercase();

    ui.columns(2, |cols| {
        cols[0].group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Users & Accounts").strong().size(13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("➕ Add User").clicked() {
                        state.selected_tab = UserManagerTab::CreateUser;
                    }
                });
            });
            ui.separator();

            let mut clicked_user = None;

            egui::ScrollArea::vertical()
                .id_salt("user_list_scroll")
                .max_height(550.0)
                .show(ui, |ui| {
                    if state.users.is_empty() && !state.is_loading {
                        ui.vertical_centered(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("⚠️ No user accounts retrieved.").strong().color(egui::Color32::from_rgb(255, 180, 80)));
                            ui.label(egui::RichText::new("Inspect the executed SQL queries below to see exact errors or permissions.").weak().size(11.0));
                            ui.add_space(8.0);
                            if ui.button("🔄 Retry Query").clicked() {
                                *out_action = Some(UserManagerAction::Refresh);
                            }
                        });
                    }

                    for (idx, user) in state.users.iter().enumerate() {
                        if !filter_text.is_empty()
                            && !user.username.to_lowercase().contains(&filter_text)
                            && !user.host.to_lowercase().contains(&filter_text)
                        {
                            continue;
                        }

                        let is_selected = state.selected_user_index == Some(idx);
                        let mut item_frame = egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .corner_radius(4.0);
                        if is_selected {
                            item_frame = item_frame.fill(ui.visuals().selection.bg_fill);
                        }

                        item_frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let icon = if user.is_superuser {
                                    "👑"
                                } else if user.can_login {
                                    "👤"
                                } else {
                                    "🛡️"
                                };
                                ui.label(egui::RichText::new(icon).size(14.0));

                                let name_text = egui::RichText::new(&user.username)
                                    .strong()
                                    .size(13.0)
                                    .color(if is_selected {
                                        ui.visuals().selection.stroke.color
                                    } else {
                                        ui.visuals().strong_text_color()
                                    });

                                if ui.selectable_label(is_selected, name_text).clicked() {
                                    clicked_user = Some((idx, user.username.clone(), user.host.clone()));
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if user.is_superuser {
                                        render_badge(ui, "SUPERUSER", egui::Color32::from_rgb(180, 120, 20));
                                    }
                                    if user.is_locked {
                                        render_badge(ui, "LOCKED", egui::Color32::from_rgb(180, 40, 40));
                                    }
                                    if !user.can_login {
                                        render_badge(ui, "ROLE/NOLOGIN", egui::Color32::from_rgb(70, 70, 120));
                                    }
                                    ui.label(
                                        egui::RichText::new(format!("@{}", user.host))
                                            .size(11.0)
                                            .weak(),
                                    );
                                });
                            });
                        });
                        ui.add_space(2.0);
                    }

                    if !state.roles.is_empty() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Database Roles / Groups").strong().size(13.0));
                        ui.separator();

                        for role in &state.roles {
                            if !filter_text.is_empty() && !role.role_name.to_lowercase().contains(&filter_text) {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("🛡️").size(13.0));
                                ui.label(egui::RichText::new(&role.role_name).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{} members", role.member_count))
                                            .size(11.0)
                                            .weak(),
                                    );
                                });
                            });
                        }
                    }
                });

            if let Some((idx, uname, uhost)) = clicked_user {
                state.selected_user_index = Some(idx);
                state.selected_grantee = Some(uname);
                state.selected_grantee_host = uhost;
                state.sync_grants_for_selected_grantee(db_type);
            }
        });

        cols[1].group(|ui| {
            if let Some(idx) = state.selected_user_index {
                if let Some(user) = state.users.get(idx).cloned() {
                    ui.horizontal(|ui| {
                        let icon = if user.is_superuser {
                            "👑"
                        } else if user.can_login {
                            "👤"
                        } else {
                            "🛡️"
                        };
                        ui.label(egui::RichText::new(icon).size(20.0));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&user.username).strong().size(16.0));
                            ui.label(
                                egui::RichText::new(format!("Host Scope: {}", user.host))
                                    .size(11.0)
                                    .weak(),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let drop_btn = egui::Button::new(
                                egui::RichText::new("🗑️ Drop User")
                                    .color(egui::Color32::from_rgb(255, 100, 100))
                                    .size(11.0),
                            );
                            if ui.add(drop_btn).clicked() {
                                *out_action = Some(UserManagerAction::OpenDropConfirmModal(
                                    user.username.clone(),
                                    user.host.clone(),
                                ));
                            }

                            let pass_btn = egui::Button::new(
                                egui::RichText::new("🔑 Change Password").size(11.0),
                            );
                            if ui.add(pass_btn).clicked() {
                                *out_action = Some(UserManagerAction::OpenChangePasswordModal(
                                    user.username.clone(),
                                    user.host.clone(),
                                ));
                            }
                        });
                    });

                    ui.separator();
                    ui.add_space(6.0);

                    ui.label(egui::RichText::new("Capabilities & Privileges").strong().size(12.0));
                    ui.add_space(2.0);

                    ui.horizontal_wrapped(|ui| {
                        if user.is_superuser {
                            render_badge(ui, "SUPERUSER / DBA", egui::Color32::from_rgb(180, 120, 20));
                        }
                        if user.can_login {
                            render_badge(ui, "CAN LOGIN", egui::Color32::from_rgb(30, 120, 60));
                        } else {
                            render_badge(ui, "NO LOGIN (ROLE)", egui::Color32::from_rgb(100, 100, 100));
                        }
                        if user.can_create_db {
                            render_badge(ui, "CAN CREATE DB", egui::Color32::from_rgb(40, 100, 160));
                        }
                        if user.can_create_role {
                            render_badge(ui, "CAN CREATE ROLE", egui::Color32::from_rgb(140, 60, 160));
                        }
                        if user.is_locked {
                            render_badge(ui, "ACCOUNT LOCKED", egui::Color32::from_rgb(180, 40, 40));
                        }
                        if user.password_expired {
                            render_badge(ui, "PASSWORD EXPIRED", egui::Color32::from_rgb(180, 80, 40));
                        }
                    });

                    if let Some(valid) = &user.valid_until {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(format!("Password Valid Until: {}", valid)).size(11.0).weak());
                    }

                    if !user.member_of.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Granted Roles / Group Memberships").strong().size(12.0));
                        ui.horizontal_wrapped(|ui| {
                            for role in &user.member_of {
                                render_badge(ui, &format!("🛡️ {}", role), egui::Color32::from_rgb(50, 80, 140));
                            }
                        });
                    }

                    if !user.attributes.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Detailed Attributes").strong().size(12.0));
                        egui::Grid::new("user_attrs_grid")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (k, v) in &user.attributes {
                                    ui.label(egui::RichText::new(k).weak().size(11.0));
                                    ui.label(egui::RichText::new(v).strong().size(11.0));
                                    ui.end_row();
                                }
                            });
                    }

                    ui.add_space(12.0);
                    let edit_grants_btn = egui::Button::new(
                        egui::RichText::new("🛡️ View / Edit Object Grants for this User")
                            .strong()
                            .size(12.0),
                    );
                    if ui.add(edit_grants_btn).clicked() {
                        state.selected_grantee = Some(user.username.clone());
                        state.selected_grantee_host = user.host.clone();
                        state.sync_grants_for_selected_grantee(db_type);
                        state.selected_tab = UserManagerTab::ObjectGrants;
                    }
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(egui::RichText::new("👈 Select a user to inspect or manage permissions").weak());
                });
            }
        });
    });
}

fn render_diagnostics_card(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    out_action: &mut Option<UserManagerAction>,
) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} Executed SQL Queries & Introspection Diagnostics", egui_icons::icons::ICON_TERMINAL.codepoint))
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().strong_text_color()),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("🔄 Re-run Queries").clicked() {
                        *out_action = Some(UserManagerAction::Refresh);
                    }
                    if ui.small_button("⚡ Open All in SQL Tab").clicked() {
                        let full_log = state.executed_queries.iter().map(|q| {
                            format!("-- Step: {}\n{};\n", q.step_name, q.sql)
                        }).collect::<Vec<String>>().join("\n");
                        *out_action = Some(UserManagerAction::OpenInSqlTab(full_log));
                    }
                    if ui.small_button("📋 Copy All Queries").clicked() {
                        let full_log = state.executed_queries.iter().map(|q| {
                            let status = match (&q.row_count, &q.error) {
                                (Some(n), _) => format!("-- [SUCCESS: {} rows]", n),
                                (_, Some(e)) => format!("-- [ERROR: {}]", e),
                                _ => "-- [UNKNOWN]".to_string(),
                            };
                            format!("-- Step: {}\n{}\n{};\n", q.step_name, status, q.sql)
                        }).collect::<Vec<String>>().join("\n");
                        ui.ctx().copy_text(full_log);
                    }
                });
            });
            ui.separator();
            ui.add_space(4.0);

            if state.executed_queries.is_empty() {
                ui.label(egui::RichText::new("No queries logged yet. Click Refresh to load.").weak().italics());
            } else {
                for (idx, q) in state.executed_queries.iter().enumerate() {
                    egui::Frame::group(ui.style())
                        .fill(ui.visuals().window_fill())
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("{}.", idx + 1)).weak().size(11.0));
                                ui.label(egui::RichText::new(&q.step_name).strong().size(12.0));

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("⚡ Open in SQL Tab / Test").clicked() {
                                        *out_action = Some(UserManagerAction::OpenInSqlTab(q.sql.clone()));
                                    }

                                    if ui.small_button("📋 Copy").clicked() {
                                        ui.ctx().copy_text(q.sql.clone());
                                    }

                                    if let Some(err) = &q.error {
                                        render_badge(ui, &format!("ERROR: {}", err), egui::Color32::from_rgb(180, 40, 40));
                                    } else if let Some(count) = q.row_count {
                                        render_badge(ui, &format!("OK: {} rows", count), egui::Color32::from_rgb(30, 120, 60));
                                    }
                                });
                            });

                            ui.add_space(2.0);
                            ui.add(
                                egui::TextEdit::multiline(&mut q.sql.as_str())
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(2),
                            );

                            if let Some(err) = &q.error {
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(format!("⚠️ Error Details: {}", err))
                                        .color(egui::Color32::from_rgb(255, 120, 120))
                                        .size(11.0),
                                );
                            }
                        });
                    ui.add_space(4.0);
                }
            }
        });
}

fn render_create_user_tab(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    db_type: Option<&DatabaseType>,
    out_action: &mut Option<UserManagerAction>,
) {
    let active_db_type = db_type.cloned().unwrap_or(DatabaseType::PostgreSQL);

    ui.columns(2, |cols| {
        cols[0].group(|ui| {
            ui.label(egui::RichText::new("➕ Create New Database User").strong().size(14.0));
            ui.separator();
            ui.add_space(6.0);

            if let Some(err) = &state.new_user_form.validation_error {
                render_status_banner(ui, err, true);
                ui.add_space(6.0);
            }

            egui::Grid::new("create_user_form_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Username:");
                    ui.text_edit_singleline(&mut state.new_user_form.username);
                    ui.end_row();

                    if active_db_type == DatabaseType::MySQL {
                        ui.label("Host Scope:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut state.new_user_form.host);
                            ui.label(egui::RichText::new("(e.g. %, localhost, 192.168.%)").size(10.0).weak());
                        });
                        ui.end_row();
                    }

                    ui.label("Password:");
                    ui.horizontal(|ui| {
                        if state.new_user_form.show_password {
                            ui.text_edit_singleline(&mut state.new_user_form.password);
                        } else {
                            ui.add(egui::TextEdit::singleline(&mut state.new_user_form.password).password(true));
                        }
                        if ui.button(if state.new_user_form.show_password { "👁" } else { "🔒" }).clicked() {
                            state.new_user_form.show_password = !state.new_user_form.show_password;
                        }
                    });
                    ui.end_row();

                    ui.label("Confirm Password:");
                    ui.add(egui::TextEdit::singleline(&mut state.new_user_form.confirm_password).password(!state.new_user_form.show_password));
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.label(egui::RichText::new("Administrative Capabilities").strong().size(12.0));
            ui.checkbox(&mut state.new_user_form.can_login, "Can Login (LOGIN)");
            ui.checkbox(&mut state.new_user_form.is_superuser, "Superuser / DBA (SUPERUSER / sysadmin)");
            ui.checkbox(&mut state.new_user_form.can_create_db, "Can Create Databases (CREATEDB)");
            ui.checkbox(&mut state.new_user_form.can_create_role, "Can Create Roles/Users (CREATEROLE)");
            ui.checkbox(&mut state.new_user_form.can_inherit, "Inherit Parent Privileges (INHERIT)");

            if !state.roles.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Assign to Roles / Groups").strong().size(12.0));
                for role in &state.roles {
                    let mut is_checked = state.new_user_form.selected_roles.contains(&role.role_name);
                    if ui.checkbox(&mut is_checked, &role.role_name).changed() {
                        if is_checked {
                            state.new_user_form.selected_roles.insert(role.role_name.clone());
                        } else {
                            state.new_user_form.selected_roles.remove(&role.role_name);
                        }
                    }
                }
            }

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let create_btn = egui::Button::new(
                    egui::RichText::new("🚀 Create User")
                        .strong()
                        .size(13.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(30, 120, 60));

                if ui.add(create_btn).clicked() {
                    if state.new_user_form.username.trim().is_empty() {
                        state.new_user_form.validation_error = Some("Username cannot be empty".to_string());
                    } else if state.new_user_form.password != state.new_user_form.confirm_password {
                        state.new_user_form.validation_error = Some("Passwords do not match".to_string());
                    } else {
                        state.new_user_form.validation_error = None;
                        *out_action = Some(UserManagerAction::CreateUser(state.new_user_form.clone()));
                    }
                }

                if ui.button("↺ Reset Form").clicked() {
                    state.new_user_form = NewUserForm::default();
                }
            });
        });

        cols[1].group(|ui| {
            ui.label(egui::RichText::new("📜 Live Generated SQL DDL").strong().size(13.0));
            ui.separator();
            ui.add_space(6.0);

            let preview_sql = generate_create_user_sql(&state.new_user_form, &active_db_type);

            egui::ScrollArea::vertical()
                .id_salt("create_user_sql_scroll")
                .max_height(400.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut preview_sql.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12),
                    );
                });

            ui.add_space(8.0);
            if ui.button("📋 Copy SQL to Clipboard").clicked() {
                ui.ctx().copy_text(preview_sql);
            }
        });
    });
}

fn render_object_grants_matrix_tab(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    db_type: Option<&DatabaseType>,
    out_action: &mut Option<UserManagerAction>,
) {
    let active_db_type = db_type.cloned().unwrap_or(DatabaseType::PostgreSQL);
    let filter_text = state.search_text.to_lowercase();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Target Grantee:").strong());

        let current_target = state
            .selected_grantee
            .clone()
            .unwrap_or_else(|| "Select User/Role".to_string());

        let mut switched_grantee = None;
        egui::ComboBox::from_id_salt("grantee_combo")
            .selected_text(format!("👤 {}", current_target))
            .show_ui(ui, |ui| {
                for user in &state.users {
                    let label = format!("👤 {}@{}", user.username, user.host);
                    let is_sel = state.selected_grantee.as_deref() == Some(&user.username);
                    if ui.selectable_label(is_sel, label).clicked() {
                        switched_grantee = Some((user.username.clone(), user.host.clone()));
                    }
                }
                for role in &state.roles {
                    let label = format!("🛡️ [Role] {}", role.role_name);
                    let is_sel = state.selected_grantee.as_deref() == Some(&role.role_name);
                    if ui.selectable_label(is_sel, label).clicked() {
                        switched_grantee = Some((role.role_name.clone(), "%".to_string()));
                    }
                }
            });

        if let Some((target, host)) = switched_grantee {
            state.selected_grantee = Some(target);
            state.selected_grantee_host = host;
            state.sync_grants_for_selected_grantee(db_type);
        }

        ui.add_space(16.0);
        ui.label("Batch Presets:");
        if ui.small_button("📖 Read-Only (SELECT)").clicked() {
            for entry in &mut state.object_grants {
                entry.has_select = true;
                entry.has_insert = false;
                entry.has_update = false;
                entry.has_delete = false;
                entry.has_all = false;
                entry.is_modified = true;
            }
        }
        if ui.small_button("✏️ Read-Write (CRUD)").clicked() {
            for entry in &mut state.object_grants {
                entry.has_select = true;
                entry.has_insert = true;
                entry.has_update = true;
                entry.has_delete = true;
                entry.has_all = false;
                entry.is_modified = true;
            }
        }
        if ui.small_button("👑 Grant ALL").clicked() {
            for entry in &mut state.object_grants {
                entry.has_select = true;
                entry.has_insert = true;
                entry.has_update = true;
                entry.has_delete = true;
                entry.has_all = true;
                entry.is_modified = true;
            }
        }
        if ui.small_button("❌ Revoke ALL").clicked() {
            for entry in &mut state.object_grants {
                entry.has_select = false;
                entry.has_insert = false;
                entry.has_update = false;
                entry.has_delete = false;
                entry.has_all = false;
                entry.is_modified = true;
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let modified_count = state.object_grants.iter().filter(|g| g.is_modified).count();
            let save_btn = egui::Button::new(
                egui::RichText::new(format!("💾 Save Changes ({})", modified_count))
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(if modified_count > 0 {
                egui::Color32::from_rgb(30, 120, 60)
            } else {
                ui.visuals().faint_bg_color
            });

            if ui.add_enabled(modified_count > 0, save_btn).clicked() {
                let grantee = state.selected_grantee.clone().unwrap_or_default();
                let grantee_host = state.selected_grantee_host.clone();

                let mut statements = Vec::new();
                for (idx, updated) in state.object_grants.iter().enumerate() {
                    if updated.is_modified {
                        let original = state.original_grants.get(idx).unwrap_or(updated);
                        let diff_sqls = generate_object_privilege_diff_sql(
                            &grantee,
                            &grantee_host,
                            original,
                            updated,
                            &active_db_type,
                        );
                        statements.extend(diff_sqls);
                    }
                }

                if !statements.is_empty() {
                    *out_action = Some(UserManagerAction::ApplyPrivilegeChanges {
                        grantee,
                        grantee_host,
                        sql_statements: statements,
                    });
                }
            }
        });
    });

    ui.separator();
    ui.add_space(4.0);

    egui::ScrollArea::both()
        .id_salt("grants_matrix_scroll")
        .max_height(550.0)
        .show(ui, |ui| {
            egui::Grid::new("grants_matrix_grid")
                .striped(true)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Schema / Database").strong());
                    ui.label(egui::RichText::new("Object Name").strong());
                    ui.label(egui::RichText::new("Type").strong());
                    ui.label(egui::RichText::new("SELECT").strong().color(egui::Color32::from_rgb(80, 180, 255)));
                    ui.label(egui::RichText::new("INSERT").strong().color(egui::Color32::from_rgb(100, 220, 120)));
                    ui.label(egui::RichText::new("UPDATE").strong().color(egui::Color32::from_rgb(255, 190, 80)));
                    ui.label(egui::RichText::new("DELETE").strong().color(egui::Color32::from_rgb(255, 100, 100)));
                    ui.label(egui::RichText::new("EXECUTE").strong().color(egui::Color32::from_rgb(200, 120, 255)));
                    ui.label(egui::RichText::new("ALL").strong());
                    ui.label(egui::RichText::new("Grant Option").strong());
                    ui.end_row();

                    for entry in &mut state.object_grants {
                        if !filter_text.is_empty()
                            && !entry.object_name.to_lowercase().contains(&filter_text)
                            && !entry.schema.to_lowercase().contains(&filter_text)
                        {
                            continue;
                        }

                        ui.label(&entry.schema);
                        ui.horizontal(|ui| {
                            if entry.is_modified {
                                ui.label(egui::RichText::new("●").color(egui::Color32::from_rgb(255, 180, 50)));
                            }
                            ui.label(egui::RichText::new(&entry.object_name).strong());
                        });

                        render_badge(
                            ui,
                            &entry.object_type,
                            if entry.object_type == "VIEW" {
                                egui::Color32::from_rgb(70, 50, 120)
                            } else {
                                egui::Color32::from_rgb(40, 70, 110)
                            },
                        );

                        if ui.checkbox(&mut entry.has_select, "").changed() { entry.is_modified = true; }
                        if ui.checkbox(&mut entry.has_insert, "").changed() { entry.is_modified = true; }
                        if ui.checkbox(&mut entry.has_update, "").changed() { entry.is_modified = true; }
                        if ui.checkbox(&mut entry.has_delete, "").changed() { entry.is_modified = true; }
                        if ui.checkbox(&mut entry.has_execute, "").changed() { entry.is_modified = true; }
                        if ui.checkbox(&mut entry.has_all, "").changed() {
                            if entry.has_all {
                                entry.has_select = true;
                                entry.has_insert = true;
                                entry.has_update = true;
                                entry.has_delete = true;
                            }
                            entry.is_modified = true;
                        }
                        if ui.checkbox(&mut entry.grant_option, "").changed() { entry.is_modified = true; }

                        ui.end_row();
                    }
                });
        });
}

fn render_sql_preview_tab(
    ui: &mut egui::Ui,
    state: &mut UserManagerState,
    out_action: &mut Option<UserManagerAction>,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("📜 Database Introspection & DDL Execution Log").strong().size(14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("🗑️ Clear Log").clicked() {
                state.generated_sql_log.clear();
            }
            if ui.button("🔄 Refresh Data & Queries").clicked() {
                *out_action = Some(UserManagerAction::Refresh);
            }
        });
    });
    ui.separator();
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .id_salt("sql_diagnostics_scroll")
        .show(ui, |ui| {
            ui.label(egui::RichText::new("🔍 Data Fetch & Introspection Queries (Background Queries)").strong().size(13.0));
            ui.label(egui::RichText::new("The queries Tabular executed to detect users, roles, table list, and permissions:").weak().size(11.0));
            ui.add_space(4.0);

            if state.executed_queries.is_empty() {
                ui.label(egui::RichText::new("No queries logged yet. Click 'Refresh Data & Queries' to execute.").weak().italics());
            } else {
                for (idx, q) in state.executed_queries.iter().enumerate() {
                    egui::Frame::group(ui.style())
                        .fill(ui.visuals().extreme_bg_color)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("#{}:", idx + 1)).weak());
                                ui.label(egui::RichText::new(&q.step_name).strong());

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("⚡ Open in SQL Tab").clicked() {
                                        *out_action = Some(UserManagerAction::OpenInSqlTab(q.sql.clone()));
                                    }
                                    if ui.small_button("📋 Copy SQL").clicked() {
                                        ui.ctx().copy_text(q.sql.clone());
                                    }
                                    if q.error.is_some() {
                                        render_badge(ui, "FAILED", egui::Color32::from_rgb(180, 40, 40));
                                    } else if let Some(n) = q.row_count {
                                        render_badge(ui, &format!("SUCCESS ({} rows)", n), egui::Color32::from_rgb(30, 120, 60));
                                    }
                                });
                            });

                            ui.add(
                                egui::TextEdit::multiline(&mut q.sql.as_str())
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY),
                            );

                            if let Some(err) = &q.error {
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(format!("⚠️ Error: {}", err))
                                        .color(egui::Color32::from_rgb(255, 120, 120))
                                        .size(11.0),
                                );
                            }
                        });
                    ui.add_space(6.0);
                }
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(6.0);

            ui.label(egui::RichText::new("⚡ Session DDL Operations (CREATE, ALTER, DROP, GRANT)").strong().size(13.0));
            ui.add_space(4.0);

            if state.generated_sql_log.is_empty() {
                ui.label(egui::RichText::new("No DDL operations executed yet in this session.").weak().italics());
            } else {
                for (idx, sql) in state.generated_sql_log.iter().enumerate() {
                    egui::Frame::group(ui.style())
                        .fill(ui.visuals().extreme_bg_color)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("DDL #{}:", idx + 1)).weak());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("📋 Copy").clicked() {
                                        ui.ctx().copy_text(sql.clone());
                                    }
                                });
                            });
                            ui.add(
                                egui::TextEdit::multiline(&mut sql.as_str())
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    ui.add_space(4.0);
                }
            }
        });
}

fn render_modals(
    ctx: &egui::Context,
    state: &mut UserManagerState,
    db_type: Option<&DatabaseType>,
    out_action: &mut Option<UserManagerAction>,
) {
    let active_db_type = db_type.cloned().unwrap_or(DatabaseType::PostgreSQL);

    if let Some(form) = &mut state.change_password_form {
        let mut close_modal = false;
        let mut submit_modal = false;

        egui::Window::new("🔑 Change User Password")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Change password for user: {}@{}", form.target_user, form.target_host));
                ui.separator();
                ui.add_space(6.0);

                if let Some(err) = &form.validation_error {
                    render_status_banner(ui, err, true);
                    ui.add_space(6.0);
                }

                egui::Grid::new("change_pass_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("New Password:");
                        ui.horizontal(|ui| {
                            if form.show_password {
                                ui.text_edit_singleline(&mut form.new_password);
                            } else {
                                ui.add(egui::TextEdit::singleline(&mut form.new_password).password(true));
                            }
                            if ui.button(if form.show_password { "👁" } else { "🔒" }).clicked() {
                                form.show_password = !form.show_password;
                            }
                        });
                        ui.end_row();

                        ui.label("Confirm Password:");
                        ui.add(egui::TextEdit::singleline(&mut form.confirm_password).password(!form.show_password));
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Save Password").clicked() {
                        if form.new_password.is_empty() {
                            form.validation_error = Some("Password cannot be empty".to_string());
                        } else if form.new_password != form.confirm_password {
                            form.validation_error = Some("Passwords do not match".to_string());
                        } else {
                            submit_modal = true;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close_modal = true;
                    }
                });
            });

        if submit_modal {
            *out_action = Some(UserManagerAction::ChangePassword(form.clone()));
            state.change_password_form = None;
        } else if close_modal {
            state.change_password_form = None;
        }
    }

    if let Some((target_user, target_host)) = &state.drop_confirm_user {
        let mut close_drop = false;
        let mut confirm_drop = false;

        egui::Window::new("⚠️ Confirm Drop User")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Are you sure you want to permanently delete user '{}'@'{}'?",
                        target_user, target_host
                    ))
                    .strong(),
                );
                ui.label(egui::RichText::new("This will revoke all granted permissions and remove access.").weak());
                ui.separator();
                ui.add_space(8.0);

                let sql_preview = generate_drop_user_sql(target_user, target_host, &active_db_type);
                ui.label(egui::RichText::new(format!("DDL: {}", sql_preview)).monospace().size(11.0));

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let del_btn = egui::Button::new(
                        egui::RichText::new("🗑️ Permanently Delete").color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(180, 30, 30));

                    if ui.add(del_btn).clicked() {
                        confirm_drop = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_drop = true;
                    }
                });
            });

        if confirm_drop {
            *out_action = Some(UserManagerAction::DropUser(
                target_user.clone(),
                target_host.clone(),
            ));
            state.drop_confirm_user = None;
        } else if close_drop {
            state.drop_confirm_user = None;
        }
    }
}

fn render_badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32) {
    egui::Frame::group(ui.style())
        .fill(fill)
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
        });
}

#[allow(dead_code)]
fn render_bool_badge(ui: &mut egui::Ui, val: bool) {
    if val {
        render_badge(ui, "YES", egui::Color32::from_rgb(30, 120, 60));
    } else {
        render_badge(ui, "NO", egui::Color32::from_rgb(90, 90, 90));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_create_user_sql_postgres() {
        let mut form = NewUserForm::default();
        form.username = "alice".to_string();
        form.password = "secret123".to_string();
        form.can_login = true;
        form.is_superuser = false;
        form.can_create_db = true;
        form.selected_roles.insert("analyst".to_string());

        let sql = generate_create_user_sql(&form, &DatabaseType::PostgreSQL);
        assert!(sql.contains("CREATE ROLE \"alice\" WITH"));
        assert!(sql.contains("PASSWORD 'secret123'"));
        assert!(sql.contains("LOGIN"));
        assert!(sql.contains("CREATEDB"));
        assert!(sql.contains("GRANT \"analyst\" TO \"alice\";"));
    }

    #[test]
    fn test_generate_create_user_sql_mysql() {
        let mut form = NewUserForm::default();
        form.username = "bob".to_string();
        form.host = "%".to_string();
        form.password = "mypass".to_string();
        form.is_superuser = true;

        let sql = generate_create_user_sql(&form, &DatabaseType::MySQL);
        assert!(sql.contains("CREATE USER 'bob'@'%' IDENTIFIED BY 'mypass';"));
        assert!(sql.contains("GRANT ALL PRIVILEGES ON *.* TO 'bob'@'%' WITH GRANT OPTION;"));
        assert!(sql.contains("FLUSH PRIVILEGES;"));
    }

    #[test]
    fn test_generate_alter_password_sql() {
        let pg_sql = generate_alter_password_sql("alice", "", "newpass", &DatabaseType::PostgreSQL);
        assert_eq!(pg_sql, "ALTER ROLE \"alice\" WITH PASSWORD 'newpass';");

        let my_sql = generate_alter_password_sql("bob", "%", "newpass", &DatabaseType::MySQL);
        assert!(my_sql.contains("ALTER USER 'bob'@'%' IDENTIFIED BY 'newpass';"));
    }

    #[test]
    fn test_generate_drop_user_sql() {
        let pg_sql = generate_drop_user_sql("alice", "", &DatabaseType::PostgreSQL);
        assert_eq!(pg_sql, "DROP ROLE \"alice\";");

        let my_sql = generate_drop_user_sql("bob", "%", &DatabaseType::MySQL);
        assert_eq!(my_sql, "DROP USER 'bob'@'%';");
    }

    #[test]
    fn test_generate_object_privilege_diff_sql() {
        let original = ObjectPrivilegeEntry {
            database: "testdb".to_string(),
            schema: "public".to_string(),
            object_name: "orders".to_string(),
            object_type: "TABLE".to_string(),
            has_select: true,
            has_insert: false,
            has_update: false,
            has_delete: false,
            has_execute: false,
            has_all: false,
            grant_option: false,
            is_modified: false,
        };

        let mut updated = original.clone();
        updated.has_insert = true;
        updated.has_update = true;

        let sqls = generate_object_privilege_diff_sql("alice", "", &original, &updated, &DatabaseType::PostgreSQL);
        assert_eq!(sqls.len(), 2);
        assert!(sqls.iter().any(|s| s.contains("GRANT INSERT ON TABLE \"public\".\"orders\" TO \"alice\";")));
        assert!(sqls.iter().any(|s| s.contains("GRANT UPDATE ON TABLE \"public\".\"orders\" TO \"alice\";")));
    }
}

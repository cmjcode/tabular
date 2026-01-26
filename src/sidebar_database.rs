use std::sync::Arc;

use eframe::egui;
use log::{debug, error, info, warn};
use sqlx::{Row, SqlitePool};

use crate::{connection, directory, models, modules, sidebar_history, window_egui};

// Helper function to determine the sort order of database types
fn database_type_order(db_type: &models::enums::DatabaseType) -> u8 {
    match db_type {
        models::enums::DatabaseType::MySQL => 0,
        models::enums::DatabaseType::PostgreSQL => 1,
        models::enums::DatabaseType::SQLite => 2,
        models::enums::DatabaseType::Redis => 3,
        models::enums::DatabaseType::MsSQL => 4,
        models::enums::DatabaseType::MongoDB => 5,
    }
}

// Helper function to sort connections in a folder by database type, then by name
fn sort_connections_in_folder(
    folder: &mut models::structs::TreeNode,
    connections: &[models::structs::ConnectionConfig],
) {
    folder.children.sort_by(|a, b| {
        // Get connection info for both nodes
        let conn_a = a
            .connection_id
            .and_then(|id| connections.iter().find(|c| c.id == Some(id)));
        let conn_b = b
            .connection_id
            .and_then(|id| connections.iter().find(|c| c.id == Some(id)));

        match (conn_a, conn_b) {
            (Some(ca), Some(cb)) => {
                // Compare by database type first, then by name
                match database_type_order(&ca.connection_type)
                    .cmp(&database_type_order(&cb.connection_type))
                {
                    std::cmp::Ordering::Equal => ca.name.cmp(&cb.name),
                    other => other,
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        }
    });
}

// Helper to parse an editable connection URL and sync it back to fields
#[derive(Debug, Clone)]
struct ParsedUrl {
    db_type: models::enums::DatabaseType,
    host: String,
    port: String,
    username: String,
    password: String,
    database: String,
}

fn parse_connection_url(input: &str) -> Option<ParsedUrl> {
    let url = input.trim();
    if url.is_empty() {
        return None;
    }

    // Handle sqlite: special cases (sqlite:path or sqlite://path)
    if let Some(rest) = url.strip_prefix("sqlite:") {
        let path = rest.strip_prefix("//").unwrap_or(rest);
        return Some(ParsedUrl {
            db_type: models::enums::DatabaseType::SQLite,
            host: path.to_string(),
            port: String::new(),
            username: String::new(),
            password: String::new(),
            database: String::new(),
        });
    }

    // General scheme:// parser
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_lowercase(), r),
        None => {
            // Accept mssql:/mysql:/postgresql:/redis: without // if user types quickly
            if let Some((s, r)) = url.split_once(':') {
                (s.to_lowercase(), r)
            } else {
                return None;
            }
        }
    };

    let db_type = match scheme.as_str() {
        "mysql" => models::enums::DatabaseType::MySQL,
        "postgres" | "postgresql" => models::enums::DatabaseType::PostgreSQL,
        "redis" => models::enums::DatabaseType::Redis,
        "mssql" | "sqlserver" => models::enums::DatabaseType::MsSQL,
        "mongodb" | "mongo" => models::enums::DatabaseType::MongoDB,
        _ => return None,
    };

    let mut user = String::new();
    let mut pass = String::new();
    let mut hostport_and_path = rest;

    // Extract auth if present: use last '@' to avoid '@' in password (should be %40 anyway)
    if let Some(at_idx) = hostport_and_path.rfind('@') {
        let (auth, after) = hostport_and_path.split_at(at_idx);
        hostport_and_path = &after[1..]; // skip '@'
        if let Some((u, p)) = auth.split_once(':') {
            user = modules::url_decode(u);
            pass = modules::url_decode(p);
        } else {
            user = modules::url_decode(auth);
        }
    }

    // Split host:port and optional /database
    let (hostport, path) = match hostport_and_path.split_once('/') {
        Some((hp, p)) => (hp, Some(p)),
        None => (hostport_and_path, None),
    };

    let host: String;
    let mut port = String::new();

    if hostport.starts_with('[') {
        // IPv6 literal [::1]:port
        if let Some(end) = hostport.find(']') {
            host = hostport[1..end].to_string();
            if let Some(rem) = hostport[end + 1..].strip_prefix(':') {
                port = rem.to_string();
            }
        } else {
            host = hostport.to_string();
        }
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        host = h.to_string();
        port = p.to_string();
    } else {
        host = hostport.to_string();
    }

    let database = path.unwrap_or("").trim_matches('/').to_string();

    // Defaults for ports if missing
    if port.is_empty() {
        port = match db_type {
            models::enums::DatabaseType::MySQL => "3306".into(),
            models::enums::DatabaseType::PostgreSQL => "5432".into(),
            models::enums::DatabaseType::Redis => "6379".into(),
            models::enums::DatabaseType::MsSQL => "1433".into(),
            models::enums::DatabaseType::SQLite => String::new(),
            models::enums::DatabaseType::MongoDB => "27017".into(),
        };
    }

    Some(ParsedUrl {
        db_type,
        host,
        port,
        username: user,
        password: pass,
        database,
    })
}

pub(crate) fn render_connection_dialog(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
    is_edit_mode: bool,
) {
    let should_show = if is_edit_mode {
        tabular.show_edit_connection
    } else {
        tabular.show_add_connection
    };

    if !should_show {
        return;
    }

    let mut open = true;
    let title = if is_edit_mode {
        "Edit Connection"
    } else {
        "Add New Connection"
    };

    // Clone the connection data to work with
    let mut connection_data = if is_edit_mode {
        tabular.edit_connection.clone()
    } else {
        tabular.new_connection.clone()
    };

    egui::Window::new(title)
        .resizable(false)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                egui::Grid::new("connection_form")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Connection Name:");
                        ui.text_edit_singleline(&mut connection_data.name);
                        ui.end_row();

                        ui.label("Database Type:");
                        egui::ComboBox::from_label("")
                            .selected_text(match connection_data.connection_type {
                                models::enums::DatabaseType::MySQL => "MySQL",
                                models::enums::DatabaseType::PostgreSQL => "PostgreSQL",
                                models::enums::DatabaseType::SQLite => "SQLite",
                                models::enums::DatabaseType::Redis => "Redis",
                                models::enums::DatabaseType::MsSQL => "MsSQL",
                                models::enums::DatabaseType::MongoDB => "MongoDB",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::MySQL,
                                    "MySQL",
                                );
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::PostgreSQL,
                                    "PostgreSQL",
                                );
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::SQLite,
                                    "SQLite",
                                );
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::Redis,
                                    "Redis",
                                );
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::MsSQL,
                                    "MsSQL",
                                );
                                ui.selectable_value(
                                    &mut connection_data.connection_type,
                                    models::enums::DatabaseType::MongoDB,
                                    "MongoDB",
                                );
                            });
                        ui.end_row();

                        if connection_data.connection_type == models::enums::DatabaseType::SQLite {
                            connection_data.ssh_enabled = false;
                        }

                        match connection_data.connection_type {
                            models::enums::DatabaseType::SQLite => {
                                // SQLite: use "Database" as file name, "Folder" as directory
                                ui.label("Database File:");
                                ui.text_edit_singleline(&mut connection_data.database);
                                ui.end_row();
                            }
                            _ => {
                                ui.label("Host:");
                                ui.text_edit_singleline(&mut connection_data.host);
                                ui.end_row();

                                ui.label("Port:");
                                ui.text_edit_singleline(&mut connection_data.port);
                                ui.end_row();

                                ui.label("Username:");
                                ui.text_edit_singleline(&mut connection_data.username);
                                ui.end_row();

                                ui.label("Password:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut connection_data.password)
                                        .password(true),
                                );
                                ui.end_row();

                                ui.label("Database:");
                                ui.text_edit_singleline(&mut connection_data.database);
                                ui.end_row();
                            }
                        }

                        // Folder field: for SQLite, used as directory picker; for others, plain text
                        ui.label("Folder (Optional):");
                        let mut folder_text = connection_data
                            .folder
                            .as_ref()
                            .unwrap_or(&String::new())
                            .clone();

                        if connection_data.connection_type == models::enums::DatabaseType::SQLite {
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut folder_text);
                                if ui.button("📂").clicked()
                                    && let Some(sender) = &tabular.background_sender
                                {
                                    let _ = sender
                                        .send(models::enums::BackgroundTask::PickSqlitePath);
                                }
                            });

                            if let Some(temp_path) = &tabular.temp_sqlite_path
                                && !temp_path.is_empty()
                            {
                                folder_text = temp_path.clone();
                            }
                        } else {
                            ui.text_edit_singleline(&mut folder_text);
                        }

                        connection_data.folder = if folder_text.trim().is_empty() {
                            None
                        } else {
                            Some(folder_text.trim().to_string())
                        };
                        ui.end_row();

                        // Build and edit Connection URL inline to keep alignment with other fields
                        let full_url = {
                            let host = connection_data.host.trim();
                            let port = connection_data.port.trim();
                            let user = connection_data.username.trim();
                            let pass = connection_data.password.clone();
                            let db = connection_data.database.trim();

                            match connection_data.connection_type {
                                models::enums::DatabaseType::MySQL => {
                                    let enc_user = modules::url_encode(user);
                                    let enc_pass = modules::url_encode(&pass);
                                    let path = if db.is_empty() {
                                        String::new()
                                    } else {
                                        format!("/{}", db)
                                    };
                                    let auth = if user.is_empty() {
                                        String::new()
                                    } else if pass.is_empty() {
                                        format!("{}@", enc_user)
                                    } else {
                                        format!("{}:{}@", enc_user, enc_pass)
                                    };
                                    format!("mysql://{}{}:{}{}", auth, host, port, path)
                                }
                                models::enums::DatabaseType::MongoDB => {
                                    let enc_user = modules::url_encode(user);
                                    let enc_pass = modules::url_encode(&pass);
                                    let auth = if user.is_empty() {
                                        String::new()
                                    } else if pass.is_empty() {
                                        format!("{}@", enc_user)
                                    } else {
                                        format!("{}:{}@", enc_user, enc_pass)
                                    };
                                    let path = if db.is_empty() {
                                        String::new()
                                    } else {
                                        format!("/{}", db)
                                    };
                                    format!("mongodb://{}{}:{}{}", auth, host, port, path)
                                }
                                models::enums::DatabaseType::PostgreSQL => {
                                    let path = if db.is_empty() {
                                        String::new()
                                    } else {
                                        format!("/{}", db)
                                    };
                                    let auth = if user.is_empty() {
                                        String::new()
                                    } else if pass.is_empty() {
                                        format!("{}@", user)
                                    } else {
                                        format!("{}:{}@", user, pass)
                                    };
                                    format!("postgresql://{}{}:{}{}", auth, host, port, path)
                                }
                                models::enums::DatabaseType::SQLite => {
                                    // For SQLite, build absolute-like path from optional folder + file name
                                    let file_name = if db.is_empty() { host } else { db };
                                    let mut path = String::new();
                                    if let Some(folder) = &connection_data.folder
                                        && !folder.is_empty()
                                    {
                                        path.push_str(folder);
                                        if !path.ends_with('/') {
                                            path.push('/');
                                        }
                                    }
                                    path.push_str(file_name);
                                    format!("sqlite:{}", path)
                                }
                                models::enums::DatabaseType::Redis => {
                                    if pass.is_empty() && user.is_empty() {
                                        format!("redis://{}:{}", host, port)
                                    } else if pass.is_empty() {
                                        format!("redis://{}@{}:{}", user, host, port)
                                    } else {
                                        format!("redis://{}:{}@{}:{}", user, pass, host, port)
                                    }
                                }
                                models::enums::DatabaseType::MsSQL => {
                                    let path = if db.is_empty() {
                                        String::new()
                                    } else {
                                        format!("/{}", db)
                                    };
                                    let auth = if user.is_empty() {
                                        String::new()
                                    } else if pass.is_empty() {
                                        format!("{}@", user)
                                    } else {
                                        format!("{}:{}@", user, pass)
                                    };
                                    format!("mssql://{}{}:{}{}", auth, host, port, path)
                                }
                            }
                        };

                        ui.label("Connection URL:");
                        let mut url_text = full_url.clone();
                        let resp = ui.text_edit_singleline(&mut url_text);
                        if resp.changed()
                            && let Some(parsed) = parse_connection_url(&url_text)
                        {
                            connection_data.connection_type = parsed.db_type;
                            connection_data.host = parsed.host;
                            connection_data.port = parsed.port;
                            connection_data.username = parsed.username;
                            connection_data.password = parsed.password;
                            connection_data.database = parsed.database;
                        }
                        ui.end_row();

                        let ssh_supported = connection_data.connection_type
                            != models::enums::DatabaseType::SQLite;

                        ui.label("SSH Tunnel:");
                        let mut ssh_checkbox_value = connection_data.ssh_enabled;
                        let ssh_checkbox = ui.add_enabled(
                            ssh_supported,
                            egui::Checkbox::new(&mut ssh_checkbox_value, "Enable SSH tunnel"),
                        );
                        if ssh_checkbox.changed() {
                            connection_data.ssh_enabled = ssh_checkbox_value;
                        }
                        if !ssh_supported {
                            if connection_data.ssh_enabled {
                                connection_data.ssh_enabled = false;
                            }
                            ui.add_enabled(false, egui::Label::new("Not available for SQLite"));
                        }
                        ui.end_row();

                        if connection_data.ssh_enabled {
                            ui.label("SSH Host:");
                            ui.text_edit_singleline(&mut connection_data.ssh_host);
                            ui.end_row();

                            ui.label("SSH Port:");
                            ui.text_edit_singleline(&mut connection_data.ssh_port);
                            ui.end_row();

                            ui.label("SSH Username:");
                            ui.text_edit_singleline(&mut connection_data.ssh_username);
                            ui.end_row();

                            ui.label("SSH Auth Method:");
                            egui::ComboBox::from_id_salt("ssh_auth_method_combo")
                                .selected_text(match connection_data.ssh_auth_method {
                                    models::enums::SshAuthMethod::Key => "Private key",
                                    models::enums::SshAuthMethod::Password => "Password",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut connection_data.ssh_auth_method,
                                        models::enums::SshAuthMethod::Key,
                                        "Private key",
                                    );
                                    ui.selectable_value(
                                        &mut connection_data.ssh_auth_method,
                                        models::enums::SshAuthMethod::Password,
                                        "Password",
                                    );
                                });
                            ui.end_row();

                            match connection_data.ssh_auth_method {
                                models::enums::SshAuthMethod::Key => {
                                    ui.label("SSH Key Path:");
                                    ui.text_edit_singleline(&mut connection_data.ssh_private_key);
                                    ui.end_row();
                                }
                                models::enums::SshAuthMethod::Password => {
                                    ui.label("SSH Password:");
                                    ui.add(
                                        egui::TextEdit::singleline(
                                            &mut connection_data.ssh_password,
                                        )
                                        .password(true),
                                    );
                                    ui.end_row();
                                }
                            }

                            ui.label("SSH Options:");
                            ui.checkbox(
                                &mut connection_data.ssh_accept_unknown_host_keys,
                                "Allow unknown host keys",
                            );
                            ui.end_row();

                            ui.label("");
                            ui.label(
                                egui::RichText::new(
                                    "Requires local ssh binary. Password mode expects sshpass to be installed.",
                                )
                                .italics(),
                            );
                            ui.end_row();
                        }
                    });

                ui.separator();

                ui.horizontal(|ui| {
                    let save_button_text = if is_edit_mode { "Update" } else { "Save" };
                    if ui.button(save_button_text).clicked() && !connection_data.name.is_empty() {
                        if is_edit_mode {
                            // Update existing connection
                            if let Some(id) = connection_data.id {
                                if connection::update_connection_in_database(
                                    tabular,
                                    &connection_data,
                                ) {
                                    load_connections(tabular);
                                    // Use incremental update instead of full rebuild
                                    update_connection_in_tree(tabular, &connection_data);
                                } else {
                                    // Fallback to in-memory update
                                    if let Some(existing) =
                                        tabular.connections.iter_mut().find(|c| c.id == Some(id))
                                    {
                                        *existing = connection_data.clone();
                                        // Use incremental update
                                        update_connection_in_tree(tabular, &connection_data);
                                    } else {
                                        debug!("ERROR: Could not find connection {} in memory", id);
                                    }
                                }
                            } else {
                                debug!("ERROR: Connection has no ID, cannot update");
                            }
                            tabular.show_edit_connection = false;
                        } else {
                            // Add new connection
                            let mut connection_to_add = connection_data.clone();

                            // Try to save to database first
                            if save_connection_to_database(tabular, &connection_to_add) {
                                // If database save successful, reload from database to get ID
                                load_connections(tabular);
                                // Find the newly added connection and add to tree incrementally
                                let added_conn = tabular
                                    .connections
                                    .iter()
                                    .find(|c| {
                                        c.name == connection_to_add.name
                                            && c.host == connection_to_add.host
                                            && c.port == connection_to_add.port
                                    })
                                    .cloned();

                                if let Some(conn) = added_conn {
                                    add_connection_to_tree(tabular, &conn);
                                }
                            } else {
                                // Fallback to in-memory storage
                                let new_id = tabular
                                    .connections
                                    .iter()
                                    .filter_map(|c| c.id)
                                    .max()
                                    .unwrap_or(0)
                                    + 1;
                                connection_to_add.id = Some(new_id);
                                tabular.connections.push(connection_to_add.clone());
                                // Add to tree incrementally
                                add_connection_to_tree(tabular, &connection_to_add);
                            }

                            tabular.new_connection = models::structs::ConnectionConfig::default();
                            tabular.test_connection_status = None;
                            tabular.test_connection_in_progress = false;
                            tabular.show_add_connection = false;
                        }
                    }

                    // Push Test Connection button ke kanan
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Test Connection button (untuk kedua mode add dan edit)
                        if tabular.test_connection_in_progress {
                            ui.spinner();
                            ui.label("Testing connection...");
                        } else if ui.button("Test Connection").clicked() {
                            // Test connection based on database type
                            // Note: This is synchronous, so we don't see the spinner. 
                            // TODO: Move to async task if spinner is needed.
                            let result = connection::test_database_connection(&connection_data);
                            tabular.test_connection_status = Some(result);
                        }
                    });
                });

                // Display test connection status (untuk kedua mode add dan edit)
                if let Some((success, message)) = &tabular.test_connection_status {
                    ui.separator();
                    if *success {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::GREEN, "✓");
                            ui.colored_label(egui::Color32::GREEN, message);
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(255, 30, 0), "✗");
                            ui.colored_label(egui::Color32::from_rgb(255, 30, 0), message);
                        });
                    }
                }
            });
        });

    // Update the original data with any changes made in the dialog
    if is_edit_mode {
        tabular.edit_connection = connection_data;
    } else {
        tabular.new_connection = connection_data;
    }

    // Handle window close via X button
    if !open {
        if is_edit_mode {
            tabular.show_edit_connection = false;
        } else {
            tabular.new_connection = models::structs::ConnectionConfig::default();
            tabular.test_connection_status = None;
            tabular.test_connection_in_progress = false;
            tabular.show_add_connection = false;
        }
    }
}

pub(crate) fn render_add_connection_dialog(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
) {
    render_connection_dialog(tabular, ctx, false);
}

pub(crate) fn render_edit_connection_dialog(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
) {
    render_connection_dialog(tabular, ctx, true);
}

pub(crate) fn load_connections(tabular: &mut window_egui::Tabular) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let connections_result = rt.block_on(async {
        sqlx::query(
            "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
             COALESCE(ssh_enabled, 0) AS ssh_enabled, \
             COALESCE(ssh_host, '') AS ssh_host, \
             COALESCE(ssh_port, '22') AS ssh_port, \
             COALESCE(ssh_username, '') AS ssh_username, \
             COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
             COALESCE(ssh_private_key, '') AS ssh_private_key, \
             COALESCE(ssh_password, '') AS ssh_password, \
             COALESCE(ssh_password, '') AS ssh_password, \
             COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys, \
             COALESCE(custom_views, '[]') AS custom_views \
         FROM connections",
        )
        .fetch_all(pool_clone.as_ref())
        .await
    });

        if let Ok(rows) = connections_result {
            tabular.connections = rows
                .into_iter()
                .filter_map(|row| {
                    let id = row.try_get::<i64, _>("id").ok()?;
                    let name = row.try_get::<String, _>("name").ok()?;
                    let host = row.try_get::<String, _>("host").ok()?;
                    let port = row.try_get::<String, _>("port").ok()?;
                    let username = row.try_get::<String, _>("username").ok()?;
                    let password = row.try_get::<String, _>("password").ok()?;
                    let database_name = row.try_get::<String, _>("database_name").ok()?;
                    let connection_type = row.try_get::<String, _>("connection_type").ok()?;
                    let folder = row.try_get::<Option<String>, _>("folder").ok()?;
                    let ssh_enabled = row.try_get::<i64, _>("ssh_enabled").ok()?;
                    let ssh_host = row.try_get::<String, _>("ssh_host").ok()?;
                    let ssh_port = row.try_get::<String, _>("ssh_port").ok()?;
                    let ssh_username = row.try_get::<String, _>("ssh_username").ok()?;
                    let ssh_auth_method = row.try_get::<String, _>("ssh_auth_method").ok()?;
                    let ssh_private_key = row.try_get::<String, _>("ssh_private_key").ok()?;
                    let ssh_password = row.try_get::<String, _>("ssh_password").ok()?;
                    let ssh_accept_unknown_host_keys =
                        row.try_get::<i64, _>("ssh_accept_unknown_host_keys").ok()?;
                    let custom_views_json = row.try_get::<String, _>("custom_views").ok().unwrap_or_else(|| "[]".to_string());

                    Some(models::structs::ConnectionConfig {
                        id: Some(id),
                        name,
                        host,
                        port,
                        username,
                        password,
                        database: database_name,
                        connection_type: match connection_type.as_str() {
                            "MySQL" => models::enums::DatabaseType::MySQL,
                            "PostgreSQL" => models::enums::DatabaseType::PostgreSQL,
                            "Redis" => models::enums::DatabaseType::Redis,
                            "MsSQL" => models::enums::DatabaseType::MsSQL,
                            "MongoDB" => models::enums::DatabaseType::MongoDB,
                            _ => models::enums::DatabaseType::SQLite,
                        },
                        folder,
                        ssh_enabled: ssh_enabled != 0,
                        ssh_host,
                        ssh_port,
                        ssh_username,
                        ssh_auth_method: models::enums::SshAuthMethod::from_db_value(
                            &ssh_auth_method,
                        ),
                        ssh_private_key,
                        ssh_password,
                        ssh_accept_unknown_host_keys: ssh_accept_unknown_host_keys != 0,
                        custom_views: serde_json::from_str(&custom_views_json).unwrap_or_default(),
                    })
                })
                .collect();
        }
    }

    // Refresh the tree after loading connections
    refresh_connections_tree(tabular);
}

pub(crate) fn save_connection_to_database(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) -> bool {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let connection = connection.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt.block_on(async {
          sqlx::query(
          "INSERT INTO connections (name, host, port, username, password, database_name, connection_type, folder, ssh_enabled, ssh_host, ssh_port, ssh_username, ssh_auth_method, ssh_private_key, ssh_password, ssh_accept_unknown_host_keys, custom_views) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
          )
          .bind(connection.name)
          .bind(connection.host)
          .bind(connection.port)
          .bind(connection.username)
          .bind(connection.password)
          .bind(connection.database)
          .bind(format!("{:?}", connection.connection_type))
          .bind(connection.folder)
          .bind(if connection.ssh_enabled { 1 } else { 0 })
          .bind(connection.ssh_host)
          .bind(connection.ssh_port)
          .bind(connection.ssh_username)
          .bind(connection.ssh_auth_method.as_db_value())
          .bind(connection.ssh_private_key)
            .bind(connection.ssh_password)
            .bind(if connection.ssh_accept_unknown_host_keys { 1 } else { 0 })
            .bind(serde_json::to_string(&connection.custom_views).unwrap_or_else(|_| "[]".to_string()))
            .execute(pool_clone.as_ref())
            .await
       });
  
          result.is_ok()
      } else {
          false
      }
  }
  
  pub(crate) fn update_connection_in_database(
      tabular: &mut window_egui::Tabular,
      connection: &models::structs::ConnectionConfig,
  ) -> bool {
      if let Some(ref pool) = tabular.db_pool {
          let pool_clone = pool.clone();
          let connection = connection.clone();
          let rt = tokio::runtime::Runtime::new().unwrap();
  
          let result = rt.block_on(async {
              sqlx::query(
                  "UPDATE connections SET name = ?, host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ?, ssh_enabled = ?, ssh_host = ?, ssh_port = ?, ssh_username = ?, ssh_auth_method = ?, ssh_private_key = ?, ssh_password = ?, ssh_accept_unknown_host_keys = ?, custom_views = ? WHERE id = ?"
              )
              .bind(connection.name)
              .bind(connection.host)
              .bind(connection.port)
              .bind(connection.username)
              .bind(connection.password)
              .bind(connection.database)
              .bind(format!("{:?}", connection.connection_type))
              .bind(connection.folder)
              .bind(if connection.ssh_enabled { 1 } else { 0 })
              .bind(connection.ssh_host)
              .bind(connection.ssh_port)
              .bind(connection.ssh_username)
              .bind(connection.ssh_auth_method.as_db_value())
              .bind(connection.ssh_private_key)
              .bind(connection.ssh_password)
              .bind(if connection.ssh_accept_unknown_host_keys { 1 } else { 0 })
              .bind(serde_json::to_string(&connection.custom_views).unwrap_or_else(|_| "[]".to_string()))
              .bind(connection.id)
              .execute(pool_clone.as_ref())
              .await
          });
  
          result.is_ok()
      } else {
          false
      }
  }

pub(crate) fn start_edit_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
    // Find the connection to edit
    if let Some(connection) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
    {
        tabular.edit_connection = connection.clone();
        // Reset test connection status saat buka edit dialog
        tabular.test_connection_status = None;
        tabular.test_connection_in_progress = false;
        tabular.show_edit_connection = true;
    } else {
        for conn in &tabular.connections {
            debug!("  - {} (ID: {:?})", conn.name, conn.id);
        }
    }
}

pub(crate) fn copy_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
    // Find the connection to copy
    if let Some(connection) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .cloned()
    {
        let mut copied_connection = connection.clone();

        // Reset ID and modify name to indicate it's a copy
        copied_connection.id = None;
        copied_connection.name = format!("{} - Copy", copied_connection.name);

        // Try to save to database first
        if save_connection_to_database(tabular, &copied_connection) {
            // If database save successful, reload from database to get ID
            load_connections(tabular);
            // Find the newly copied connection and add to tree incrementally
            let added_conn = tabular
                .connections
                .iter()
                .find(|c| {
                    c.name == copied_connection.name
                        && c.host == copied_connection.host
                        && c.port == copied_connection.port
                })
                .cloned();

            if let Some(conn) = added_conn {
                add_connection_to_tree(tabular, &conn);
            }
        } else {
            // Fallback to in-memory storage
            let new_id = tabular
                .connections
                .iter()
                .filter_map(|c| c.id)
                .max()
                .unwrap_or(0)
                + 1;
            copied_connection.id = Some(new_id);
            tabular.connections.push(copied_connection.clone());
            // Add to tree incrementally
            add_connection_to_tree(tabular, &copied_connection);
        }
    } else {
        debug!(
            "❌ Connection with ID {} not found for copying",
            connection_id
        );
    }
}

pub(crate) fn initialize_database(tabular: &mut window_egui::Tabular) {
    // Ensure app directories exist
    if let Err(e) = directory::ensure_app_directories() {
        error!("Failed to create app directories: {}", e);
        return;
    }

    // Initialize SQLite database
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool_result = rt.block_on(async {
            // Get the data directory path
            let data_dir = directory::get_data_dir();
            let db_path = data_dir.join("connections.db");

            // Convert path to string and use file:// prefix for SQLite
            let db_path_str = db_path.to_string_lossy();
            let connection_string = format!("sqlite://{}?mode=rwc", db_path_str);
            let pool = SqlitePool::connect(&connection_string).await;

            match pool {
                Ok(pool) => {
                    info!("Database connection successful");

                    // Create connections table
                    let create_connections_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS connections (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            name TEXT NOT NULL,
                            host TEXT NOT NULL,
                            port TEXT NOT NULL,
                            username TEXT NOT NULL,
                            password TEXT NOT NULL,
                            database_name TEXT NOT NULL,
                            connection_type TEXT NOT NULL,
                            folder TEXT DEFAULT NULL,
                            ssh_enabled INTEGER NOT NULL DEFAULT 0,
                            ssh_host TEXT NOT NULL DEFAULT '',
                            ssh_port TEXT NOT NULL DEFAULT '22',
                            ssh_username TEXT NOT NULL DEFAULT '',
                            ssh_auth_method TEXT NOT NULL DEFAULT 'key',
                            ssh_private_key TEXT NOT NULL DEFAULT '',
                            ssh_password TEXT NOT NULL DEFAULT '',
                            ssh_accept_unknown_host_keys INTEGER NOT NULL DEFAULT 0,
                            custom_views TEXT NOT NULL DEFAULT '[]'
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Add folder column if it doesn't exist (for existing databases)
                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN folder TEXT DEFAULT NULL"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_enabled INTEGER NOT NULL DEFAULT 0"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_host TEXT NOT NULL DEFAULT ''"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_port TEXT NOT NULL DEFAULT '22'"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_username TEXT NOT NULL DEFAULT ''"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN custom_views TEXT NOT NULL DEFAULT '[]'"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_auth_method TEXT NOT NULL DEFAULT 'key'"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_private_key TEXT NOT NULL DEFAULT ''"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_password TEXT NOT NULL DEFAULT ''"
                    )
                    .execute(&pool)
                    .await;

                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN ssh_accept_unknown_host_keys INTEGER NOT NULL DEFAULT 0"
                    )
                    .execute(&pool)
                    .await;


                    // Create database cache table
                    let create_db_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS database_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create table cache table
                    let create_table_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS table_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            table_type TEXT NOT NULL, -- 'table', 'view', 'procedure', etc.
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, table_type)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create column cache table
                    let create_column_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS column_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            column_name TEXT NOT NULL,
                            data_type TEXT NOT NULL,
                            ordinal_position INTEGER NOT NULL,
                            -- New flags for schema insights
                            is_primary_key INTEGER NOT NULL DEFAULT 0, -- 0 = false, 1 = true
                            is_indexed INTEGER NOT NULL DEFAULT 0,     -- 0 = false, 1 = true
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, column_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create query history table
                    let create_history_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS query_history (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            query_text TEXT NOT NULL,
                            connection_id INTEGER NOT NULL,
                            connection_name TEXT NOT NULL,
                            executed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create row cache table for cached table data (first 100 rows)
                    let create_row_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS row_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            headers_json TEXT NOT NULL,
                            rows_json TEXT NOT NULL,
                            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create index cache table for cached index metadata
                    let create_index_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS index_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            index_name TEXT NOT NULL,
                            method TEXT NULL,
                            is_unique INTEGER NOT NULL DEFAULT 0,
                            columns_json TEXT NOT NULL DEFAULT '[]',
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, index_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    // Create partition cache table for cached partition metadata
                    let create_partition_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS partition_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            partition_name TEXT NOT NULL,
                            partition_type TEXT NULL,
                            partition_expression TEXT NULL,
                            subpartition_type TEXT NULL,
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, partition_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;

                    match (create_connections_result, create_db_cache_result, create_table_cache_result, create_column_cache_result, create_history_result, create_row_cache_result, create_index_cache_result, create_partition_cache_result) {
                        (Ok(_), Ok(_), Ok(_), Ok(_), Ok(_), Ok(_), Ok(_), Ok(_)) => {
                            Some(pool)
                        },
                        _ => {
                            warn!("Error creating some tables");
                            None
                        }
                    }
                },
                Err(e) => {
                    error!("Database connection failed: {}", e);
                    None
                }
            }
        });

    if let Some(pool) = pool_result {
        tabular.db_pool = Some(Arc::new(pool));
        // Best-effort migrations for new columns (idempotent): add flags to column_cache
        // Ignore errors if columns already exist
        if let Some(ref pool) = tabular.db_pool {
            rt.block_on(async {
                let _ = sqlx::query(
                    "ALTER TABLE column_cache ADD COLUMN is_primary_key INTEGER NOT NULL DEFAULT 0",
                )
                .execute(pool.as_ref())
                .await;
                let _ = sqlx::query(
                    "ALTER TABLE column_cache ADD COLUMN is_indexed INTEGER NOT NULL DEFAULT 0",
                )
                .execute(pool.as_ref())
                .await;
            });
        }
        // Load existing connections from database
        load_connections(tabular);
        // Load query history from database
        sidebar_history::load_query_history(tabular);
    }
}

pub(crate) fn initialize_sample_data(tabular: &mut window_egui::Tabular) {
    // Initialize with connections as root nodes
    refresh_connections_tree(tabular);

    // Don't add sample queries - let load_queries_from_directory handle the real structure
    // self.queries_tree will be populated by load_queries_from_directory()

    // Initialize empty history tree (will be loaded from database)
    //  self.refresh_history_tree();
}

pub(crate) fn refresh_connections_tree(tabular: &mut window_egui::Tabular) {
    // Save current expansion states before rebuilding
    let expansion_states = save_tree_expansion_states(&tabular.items_tree);

    // Clear existing tree
    tabular.items_tree.clear();

    // Create folder structure for connections
    tabular.items_tree = create_connections_folder_structure(tabular);

    // Restore expansion states
    restore_tree_expansion_states(&mut tabular.items_tree, &expansion_states);
}

// Helper to save expansion states recursively
fn save_tree_expansion_states(
    tree: &[models::structs::TreeNode],
) -> std::collections::HashMap<String, bool> {
    let mut states = std::collections::HashMap::new();

    fn collect_states(
        node: &models::structs::TreeNode,
        states: &mut std::collections::HashMap<String, bool>,
        path: String,
    ) {
        if node.is_expanded {
            states.insert(path.clone(), true);
        }
        for child in &node.children {
            let child_path = if path.is_empty() {
                child.name.clone()
            } else {
                format!("{}>{}", path, child.name)
            };
            collect_states(child, states, child_path);
        }
    }

    for node in tree {
        collect_states(node, &mut states, node.name.clone());
    }

    states
}

// Helper to restore expansion states recursively
fn restore_tree_expansion_states(
    tree: &mut [models::structs::TreeNode],
    states: &std::collections::HashMap<String, bool>,
) {
    fn restore_states(
        node: &mut models::structs::TreeNode,
        states: &std::collections::HashMap<String, bool>,
        path: String,
    ) {
        if let Some(&expanded) = states.get(&path) {
            node.is_expanded = expanded;
        }
        for child in &mut node.children {
            let child_path = if path.is_empty() {
                child.name.clone()
            } else {
                format!("{}>{}", path, child.name)
            };
            restore_states(child, states, child_path);
        }
    }

    for node in tree {
        restore_states(node, states, node.name.clone());
    }
}

// Incremental update: Add a new connection to the tree without full rebuild
pub(crate) fn add_connection_to_tree(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) {
    if let Some(id) = connection.id {
        let folder_name = connection
            .folder
            .as_ref()
            .unwrap_or(&"Default".to_string())
            .clone();

        let display_name = format!("{} {}", connection.connection_type.icon(), connection.name);
        let new_node = models::structs::TreeNode::new_connection(display_name, id);

        // Find or create the folder
        if let Some(folder) = tabular
            .items_tree
            .iter_mut()
            .find(|n| n.name == folder_name)
        {
            // Add to existing folder, maintaining sort order by database type
            folder.children.push(new_node);
            sort_connections_in_folder(folder, &tabular.connections);
        } else {
            // Create new folder
            let mut new_folder = models::structs::TreeNode::new(
                folder_name.clone(),
                models::enums::NodeType::CustomFolder,
            );
            new_folder.children.push(new_node);
            tabular.items_tree.push(new_folder);

            // Re-sort folders
            tabular.items_tree.sort_by(|a, b| {
                if a.name == "Default" {
                    std::cmp::Ordering::Less
                } else if b.name == "Default" {
                    std::cmp::Ordering::Greater
                } else {
                    a.name.cmp(&b.name)
                }
            });
        }
    }
}

// Incremental update: Update an existing connection in the tree
pub(crate) fn update_connection_in_tree(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) {
    if let Some(id) = connection.id {
        let new_folder = connection
            .folder
            .as_ref()
            .unwrap_or(&"Default".to_string())
            .clone();

        let new_display_name = format!("{} {}", connection.connection_type.icon(), connection.name);

        // Find and remove the old node (might be in different folder)
        let mut old_node_state: Option<(models::structs::TreeNode, String)> = None;

        for folder in &mut tabular.items_tree {
            if let Some(pos) = folder
                .children
                .iter()
                .position(|n| n.connection_id == Some(id))
            {
                old_node_state = Some((folder.children.remove(pos), folder.name.clone()));
                break;
            }
        }

        // Create updated node, preserving expansion state
        let mut updated_node = models::structs::TreeNode::new_connection(new_display_name, id);
        if let Some((old_node, old_folder)) = old_node_state {
            // Preserve expansion state and children if expanded
            updated_node.is_expanded = old_node.is_expanded;
            updated_node.is_loaded = old_node.is_loaded;
            updated_node.children = old_node.children;

            // Add to the new folder
            if let Some(folder) = tabular.items_tree.iter_mut().find(|n| n.name == new_folder) {
                folder.children.push(updated_node);
                sort_connections_in_folder(folder, &tabular.connections);
            } else {
                // Create new folder if it doesn't exist
                let mut new_folder_node = models::structs::TreeNode::new(
                    new_folder.clone(),
                    models::enums::NodeType::CustomFolder,
                );
                new_folder_node.children.push(updated_node);
                tabular.items_tree.push(new_folder_node);

                // Re-sort folders
                tabular.items_tree.sort_by(|a, b| {
                    if a.name == "Default" {
                        std::cmp::Ordering::Less
                    } else if b.name == "Default" {
                        std::cmp::Ordering::Greater
                    } else {
                        a.name.cmp(&b.name)
                    }
                });
            }

            // Clean up empty folder if old folder is now empty
            if old_folder != new_folder
                && let Some(pos) = tabular
                    .items_tree
                    .iter()
                    .position(|f| f.name == old_folder && f.children.is_empty())
            {
                tabular.items_tree.remove(pos);
            }
        }
    }
}

// Incremental update: Remove a connection from the tree
pub(crate) fn remove_connection_from_tree(tabular: &mut window_egui::Tabular, connection_id: i64) {
    // Find and remove the connection node
    for folder in &mut tabular.items_tree {
        if let Some(pos) = folder
            .children
            .iter()
            .position(|n| n.connection_id == Some(connection_id))
        {
            folder.children.remove(pos);
            break;
        }
    }

    // Remove empty folders (except Default)
    tabular
        .items_tree
        .retain(|folder| !folder.children.is_empty() || folder.name == "Default");
}

pub fn get_default_dba_views(
    driver: &models::enums::DatabaseType,
) -> Vec<(&'static str, models::enums::NodeType, &'static str)> {
    use models::enums::{DatabaseType, NodeType};
    match driver {
        DatabaseType::MySQL => vec![
            (
                "Users",
                NodeType::UsersFolder,
                "SELECT Host, User, plugin, account_locked, password_expired, password_last_changed FROM mysql.user ORDER BY User, Host;"
            ),
            (
                "Privileges",
                NodeType::PrivilegesFolder,
                "SELECT GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USER_PRIVILEGES ORDER BY GRANTEE, PRIVILEGE_TYPE;"
            ),
            (
                "Processes",
                NodeType::ProcessesFolder,
                "SHOW FULL PROCESSLIST;"
            ),
            (
                "Status",
                NodeType::StatusFolder,
                "SHOW GLOBAL STATUS;"
            ),
            (
                "Blocked Query",
                NodeType::BlockedQueriesFolder,
                "SELECT * FROM information_schema.PROCESSLIST WHERE STATE LIKE '%lock%';"
            ),
            (
                "Replication Status",
                NodeType::ReplicationStatusFolder,
                "SHOW REPLICA STATUS;"
            ),
            (
                "Master Status",
                NodeType::MasterStatusFolder,
                "SHOW MASTER STATUS;"
            ),
            (
                "User Active",
                NodeType::MetricsUserActiveFolder,
                "SELECT USER, COUNT(*) AS session_count FROM information_schema.PROCESSLIST GROUP BY USER ORDER BY session_count DESC;"
            ),
        ],
        DatabaseType::PostgreSQL => vec![
            (
                "Users",
                NodeType::UsersFolder,
                "SELECT usename AS user, usesysid, usecreatedb, usesuper FROM pg_user ORDER BY usename;"
            ),
            (
                "Privileges",
                NodeType::PrivilegesFolder,
                "SELECT grantee, table_catalog, table_schema, table_name, privilege_type FROM information_schema.table_privileges ORDER BY grantee, table_schema, table_name;"
            ),
            (
                "Processes",
                NodeType::ProcessesFolder,
                "SELECT pid, usename, application_name, client_addr, state, query_start, query FROM pg_stat_activity ORDER BY query_start DESC NULLS LAST;"
            ),
            (
                "Status",
                NodeType::StatusFolder,
                "SELECT name, setting FROM pg_settings ORDER BY name;"
            ),
            (
                "Blocked Query",
                NodeType::BlockedQueriesFolder,
                "SELECT\n    blocked.pid AS blocked_pid,\n    blocked.usename AS blocked_user,\n    blocked.application_name AS blocked_app,\n    blocked.client_addr AS blocked_client,\n    blocked.wait_event_type,\n    blocked.wait_event,\n    blocked.query_start AS blocked_query_start,\n    blocked.query AS blocked_query,\n    blocking.pid AS blocking_pid,\n    blocking.usename AS blocking_user,\n    blocking.application_name AS blocking_app,\n    blocking.client_addr AS blocking_client,\n    blocking.query_start AS blocking_query_start,\n    blocking.query AS blocking_query\nFROM pg_stat_activity blocked\nJOIN pg_locks blocked_locks ON blocked.pid = blocked_locks.pid AND NOT blocked_locks.granted\nJOIN pg_locks blocking_locks ON blocking_locks.locktype = blocked_locks.locktype\n    AND blocking_locks.database IS NOT DISTINCT FROM blocked_locks.database\n    AND blocking_locks.relation IS NOT DISTINCT FROM blocked_locks.relation\n    AND blocking_locks.page IS NOT DISTINCT FROM blocked_locks.page\n    AND blocking_locks.tuple IS NOT DISTINCT FROM blocked_locks.tuple\n    AND blocking_locks.virtualxid IS NOT DISTINCT FROM blocked_locks.virtualxid\n    AND blocking_locks.transactionid IS NOT DISTINCT FROM blocked_locks.transactionid\n    AND blocking_locks.classid IS NOT DISTINCT FROM blocked_locks.classid\n    AND blocking_locks.objid IS NOT DISTINCT FROM blocked_locks.objid\n    AND blocking_locks.objsubid IS NOT DISTINCT FROM blocked_locks.objsubid\nJOIN pg_stat_activity blocking ON blocking.pid = blocking_locks.pid\nWHERE blocked.wait_event_type IS NOT NULL\nORDER BY blocked.query_start;"
            ),
            (
                "User Active",
                NodeType::MetricsUserActiveFolder,
                "SELECT usename AS user, COUNT(*) AS session_count FROM pg_stat_activity GROUP BY usename ORDER BY session_count DESC;"
            ),
        ],
        DatabaseType::MsSQL => vec![
            (
                "Users",
                NodeType::UsersFolder,
                "SELECT name, type_desc, create_date, modify_date FROM sys.server_principals WHERE type IN ('S','U','G') AND name NOT LIKE '##MS_%' ORDER BY name;"
            ),
            (
                "Privileges",
                NodeType::PrivilegesFolder,
                "SELECT dp.name AS principal_name, sp.permission_name, sp.state_desc FROM sys.server_permissions sp JOIN sys.server_principals dp ON sp.grantee_principal_id = dp.principal_id ORDER BY dp.name, sp.permission_name;"
            ),
            (
                "Processes",
                NodeType::ProcessesFolder,
                "SELECT session_id, login_name, host_name, status, program_name, cpu_time, memory_usage FROM sys.dm_exec_sessions ORDER BY cpu_time DESC;"
            ),
            (
                "Status",
                NodeType::StatusFolder,
                "SELECT TOP 200 counter_name, instance_name, cntr_value FROM sys.dm_os_performance_counters ORDER BY counter_name;"
            ),
            (
                "Blocked Query",
                NodeType::BlockedQueriesFolder,
                "SELECT\n    blocked_req.session_id AS blocked_session_id,\n    blocked.login_name AS blocked_login,\n    blocked.status AS blocked_status,\n    blocked_req.wait_time AS blocked_wait_ms,\n    blocked_req.last_wait_type AS blocked_last_wait_type,\n    DB_NAME(blocked_req.database_id) AS database_name,\n    blocked_text.text AS blocked_query,\n    blocked_req.blocking_session_id AS blocking_session_id,\n    blocking.login_name AS blocking_login,\n    blocking.status AS blocking_status,\n    blocking_text.text AS blocking_query\nFROM sys.dm_exec_requests blocked_req\nJOIN sys.dm_exec_sessions blocked ON blocked_req.session_id = blocked.session_id\nLEFT JOIN sys.dm_exec_sessions blocking ON blocked_req.blocking_session_id = blocking.session_id\nLEFT JOIN sys.dm_exec_requests blocking_req ON blocked_req.blocking_session_id = blocking_req.session_id\nOUTER APPLY sys.dm_exec_sql_text(blocked_req.sql_handle) AS blocked_text\nOUTER APPLY sys.dm_exec_sql_text(blocking_req.sql_handle) AS blocking_text\nWHERE blocked_req.blocking_session_id <> 0\nORDER BY blocked_req.wait_time DESC;"
            ),
            (
                "User Active",
                NodeType::MetricsUserActiveFolder,
                "SELECT login_name AS [user], COUNT(*) AS session_count FROM sys.dm_exec_sessions GROUP BY login_name ORDER BY session_count DESC;"
            ),
        ],
        _ => vec![],
    }
}

pub(crate) fn create_connections_folder_structure(
    tabular: &mut window_egui::Tabular,
) -> Vec<models::structs::TreeNode> {
    // Group connections by custom folder first
    let mut folder_groups: std::collections::HashMap<
        String,
        Vec<&models::structs::ConnectionConfig>,
    > = std::collections::HashMap::new();

    // Group connections by custom folder
    for conn in &tabular.connections {
        let folder_name = conn
            .folder
            .as_ref()
            .unwrap_or(&"Default".to_string())
            .clone();
        folder_groups.entry(folder_name).or_default().push(conn);
    }

    let mut result = Vec::new();

    // Create folder structure for each custom folder
    for (folder_name, connections) in folder_groups {
        if connections.is_empty() {
            continue;
        }

        // Create custom folder node
        let mut custom_folder = models::structs::TreeNode::new(
            folder_name.clone(),
            models::enums::NodeType::CustomFolder,
        );
        custom_folder.is_expanded = false; // Start collapsed

        // Add connections directly under the folder with database type icon
        let mut folder_connections = Vec::new();

        for conn in connections {
            if let Some(id) = conn.id {
                // Create display name with database icon
                let display_name = format!("{} {}", conn.connection_type.icon(), conn.name);
                let node = models::structs::TreeNode::new_connection(display_name, id);
                folder_connections.push((node, conn.connection_type.clone(), conn.name.clone()));
            } else {
                debug!("  -> Skipping connection with no ID");
            }
        }

        // Sort connections by database type first, then by name
        folder_connections.sort_by(|a, b| {
            match database_type_order(&a.1).cmp(&database_type_order(&b.1)) {
                std::cmp::Ordering::Equal => a.2.cmp(&b.2),
                other => other,
            }
        });

        custom_folder.children = folder_connections
            .into_iter()
            .map(|(node, _, _)| node)
            .collect();
        result.push(custom_folder);
    }

    // Sort folders alphabetically, but put "Default" first
    result.sort_by(|a, b| {
        if a.name == "Default" {
            std::cmp::Ordering::Less
        } else if b.name == "Default" {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    if result.is_empty() {
        debug!("No connections found, returning empty tree");
    }

    result
}

use eframe::egui;
use crate::models;

impl super::Tabular {
    pub fn get_diagram_path(&self, conn_id: i64, db_name: &str) -> Option<std::path::PathBuf> {
        let mut path = if !self.data_directory.is_empty() {
             std::path::PathBuf::from(&self.data_directory).join("diagrams")
        } else if let Some(config_dir) = dirs::data_local_dir() {
             config_dir.join("tabular").join("diagrams")
        } else {
             return None;
        };

        let _ = std::fs::create_dir_all(&path);
        // Sanitize filename
        let safe_db_name: String = db_name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        path.push(format!("conn_{}_{}.json", conn_id, safe_db_name));
        log::debug!("get_diagram_path: inputs=({}, '{}') -> path={:?}", conn_id, db_name, path);
        Some(path)
    }
    pub fn render_cache_miss_dialog(&mut self, ctx: &egui::Context) {
        if let Some((conn_id, db_name, table_name)) = &self.cache_miss_request {
             let mut open = true;
             let mut confirmed = false;
             let mut should_close = false;
             
             egui::Window::new("Metadata Missing")
                 .open(&mut open)
                 .collapsible(false)
                 .resizable(false)
                 .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                 .show(ctx, |ui| {
                     ui.label(format!("Metadata for table '{}' is not in cache.", table_name));
                     ui.label("Would you like to fetch it now?");
                     ui.add_space(10.0);
                     ui.horizontal(|ui| {
                         if ui.button("Fetch Metadata").clicked() {
                             confirmed = true;
                         }
                         if ui.button("Cancel").clicked() {
                             should_close = true;
                         }
                     });
                 });
             
             if should_close {
                 open = false;
             }
             
             if confirmed {
                 // Trigger background fetch
                 let conn_id = *conn_id;
                 let db = db_name.clone();
                 let table = table_name.clone();
                 
                 // We can use existing function connection::fetch_columns_from_database
                 // avoiding async generic hell by doing it in the background thread if possible, 
                 // or just spawning a tokio task here since we have runtime.
                 // or just spawning a tokio task here since we have runtime.
                 if let Some(rt) = self.runtime.clone()
                     && let Some(conn_config) = self
                         .connections
                         .iter()
                         .find(|c| c.id == Some(conn_id))
                         .cloned()
                 {
                     let pool_clone = self.db_pool.clone();
                     rt.spawn(async move {
                         // Fetch columns
                         if let Some(cols) = crate::connection::fetch_columns_from_database(
                             conn_id,
                             &db,
                             &table,
                             &conn_config,
                         ) {
                             // This requires a mutable Tabular reference to save to cache, which we don't have easily in async.
                             // But `save_columns_to_cache` mainly needs db_pool.
                             // Let's manually call sqlx logic or refactor save_columns_to_cache to not need Tabular.
                             // Attempting direct sqlx insert matching cache_data logic:
                             if let Some(pool) = pool_clone {
                                 // Copy-paste save logic or make it cleaner later. 
                                 // For now, let's just use the existing function if we can refactor it.
                                 // Refactoring `save_columns_to_cache` to take `&SqlitePool` is best.
                                 // But I cannot refactor it right now easily without breaking other calls.
                                 // So I will implement a raw SQL insert here for the fix.

                                 println!("🔥 Loading columns for {}...", table);
                                 // CLEAR
                                 let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                                     .bind(conn_id)
                                     .bind(&db)
                                     .bind(&table)
                                     .execute(pool.as_ref())
                                     .await;

                                 // INSERT
                                 for (i, (cname, ctype)) in cols.iter().enumerate() {
                                     let _ = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
                                         .bind(conn_id)
                                         .bind(&db)
                                         .bind(&table)
                                         .bind(cname)
                                         .bind(ctype)
                                         .bind(i as i64)
                                         .execute(pool.as_ref())
                                         .await;
                                 }
                                 println!("🔥 Columns loaded for {}", table);
                             }
                         }
                     });
                 }

                 self.cache_miss_request = None;
             } else if !open {
                 self.cache_miss_request = None;
             }
        }
    }
    pub fn save_diagram(&self, conn_id: i64, db_name: &str, state: &models::structs::DiagramState) {
        if let Some(path) = self.get_diagram_path(conn_id, db_name) {
            match std::fs::File::create(&path) {
                Ok(file) => {
                    let writer = std::io::BufWriter::new(file);
                    if let Err(e) = serde_json::to_writer_pretty(writer, state) {
                        log::error!("Failed to serialize diagram state: {}", e);
                    } else {
                        log::info!("Diagram layout saved to {:?}", path);
                    }
                },
                Err(e) => log::error!("Failed to create diagram file {:?}: {}", path, e),
            }
        }
    }
    pub fn load_diagram(&self, conn_id: i64, db_name: &str) -> Option<models::structs::DiagramState> {
        if let Some(path) = self.get_diagram_path(conn_id, db_name)
            && path.exists() {
                 match std::fs::File::open(&path) {
                    Ok(file) => {
                        let reader = std::io::BufReader::new(file);
                        match serde_json::from_reader(reader) {
                            Ok(state) => {
                                log::info!("Diagram layout loaded from {:?}", path);
                                return Some(state);
                            },
                            Err(e) => log::error!("Failed to deserialize diagram state: {}", e),
                        }
                    },
                    Err(e) => log::error!("Failed to open diagram file {:?}: {}", path, e),
                 }
                 None
            } else {
                None
            }
    }
}

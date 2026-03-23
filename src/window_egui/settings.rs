use crate::directory;

impl super::Tabular {
    pub fn handle_directory_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.directory_picker_result = Some(receiver);

        // Spawn directory picker in a separate thread to avoid blocking UI
        let current_dir = self.data_directory.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih Lokasi Data Directory")
                .set_directory(&current_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
        self.show_directory_picker = false;
    }

    // Handle SQLite file/folder picker for new connection dialog
    #[allow(dead_code)]
    pub(crate) fn handle_sqlite_path_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.sqlite_path_picker_result = Some(receiver);

        let default_dir = if !self.data_directory.is_empty() {
            self.data_directory.clone()
        } else {
            crate::config::get_data_dir().to_string_lossy().to_string()
        };

        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih File / Folder SQLite")
                .set_directory(&default_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
    }
    pub(crate) fn handle_save_directory_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.save_directory_picker_result = Some(receiver);

        // Spawn directory picker in a separate thread to avoid blocking UI
        let default_dir = if !self.save_directory.is_empty() {
            self.save_directory.clone()
        } else {
            crate::directory::get_query_dir()
                .to_string_lossy()
                .to_string()
        };

        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih Lokasi Penyimpanan Query")
                .set_directory(&default_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
    }
    pub fn refresh_data_directory(&mut self) {
        self.data_directory = crate::config::get_data_dir().to_string_lossy().to_string();
    }
}

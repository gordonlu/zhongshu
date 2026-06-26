use std::path::PathBuf;

pub struct WorkingSet {
    pub read_files: Vec<PathBuf>,
    pub modified_files: Vec<PathBuf>,
}

impl WorkingSet {
    pub fn new() -> Self {
        WorkingSet {
            read_files: Vec::new(),
            modified_files: Vec::new(),
        }
    }

    pub fn record_read(&mut self, path: PathBuf) {
        if !self.read_files.contains(&path) {
            self.read_files.push(path);
        }
    }

    pub fn record_modify(&mut self, path: PathBuf) {
        if !self.modified_files.contains(&path) {
            self.modified_files.push(path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct PatchHistory {
    pub diff_hashes: Vec<String>,
}

impl PatchHistory {
    pub fn new() -> Self {
        PatchHistory {
            diff_hashes: Vec::new(),
        }
    }

    pub fn record(&mut self, diff: &str) {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        diff.hash(&mut hasher);
        let hash = format!("{:x}", hasher.finish());
        self.diff_hashes.push(hash);
    }

    pub fn is_repeated(&self) -> bool {
        if self.diff_hashes.len() < 3 {
            return false;
        }
        let last = &self.diff_hashes[self.diff_hashes.len() - 1];
        let second_last = &self.diff_hashes[self.diff_hashes.len() - 2];
        let third_last = &self.diff_hashes[self.diff_hashes.len() - 3];
        last == second_last && second_last == third_last
    }
}

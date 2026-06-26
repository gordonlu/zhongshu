use crate::harness::architecture::index::ProjectIndex;

pub fn render_project_map(index: &ProjectIndex) -> String {
    let mut lines = Vec::new();
    lines.push("## 项目结构".to_string());

    let mut dirs: Vec<_> = index.files.keys().collect();
    dirs.sort();

    for path in &dirs {
        let display = path.strip_prefix(&index.root).unwrap_or(path);
        if let Some(file_index) = index.files.get(*path) {
            let items: Vec<&str> = file_index.items.iter().map(|s| s.as_str()).collect();
            if !items.is_empty() {
                lines.push(format!("  {} — {}", display.display(), items.join(", ")));
            }
        }
    }
    lines.join("\n")
}

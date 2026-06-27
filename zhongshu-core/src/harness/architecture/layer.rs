use std::path::Path;

/// Map a file path to its layer name.
pub struct LayerGraph {
    pub layers: Vec<(String, globset::GlobMatcher)>,
}

impl LayerGraph {
    pub fn new() -> Self {
        LayerGraph { layers: Vec::new() }
    }

    pub fn add_layer(&mut self, name: &str, glob: &str) {
        let matcher = crate::harness::architecture::config::glob_matcher(glob);
        self.layers.push((name.to_string(), matcher));
    }

    pub fn layer_for(&self, path: &Path) -> Option<&str> {
        for (name, matcher) in &self.layers {
            if matcher.is_match(path) {
                return Some(name);
            }
            if path.is_absolute() {
                if let Ok(cwd) = std::env::current_dir() {
                    if let Ok(relative) = path.strip_prefix(cwd) {
                        if matcher.is_match(relative) {
                            return Some(name);
                        }
                    }
                }
            }
        }
        None
    }

    /// Build default layer graph for zhongshu project.
    pub fn default() -> Self {
        let mut g = LayerGraph::new();
        g.add_layer("orb", "zhongshu-orb/src/**/*.rs");
        g.add_layer("core", "zhongshu-core/src/**/*.rs");
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layers_match_relative_paths() {
        let layers = LayerGraph::default();
        assert_eq!(
            layers.layer_for(Path::new("zhongshu-core/src/lib.rs")),
            Some("core")
        );
        assert_eq!(
            layers.layer_for(Path::new("zhongshu-orb/src/main.rs")),
            Some("orb")
        );
    }

    #[test]
    fn default_layers_match_absolute_paths_under_cwd() {
        let layers = LayerGraph::default();
        let path = std::env::current_dir()
            .unwrap()
            .join("zhongshu-core/src/lib.rs");
        assert_eq!(layers.layer_for(&path), Some("core"));
    }
}

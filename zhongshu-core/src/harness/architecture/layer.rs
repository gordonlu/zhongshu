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

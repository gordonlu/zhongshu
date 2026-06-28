use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const LEGACY_CHAT_HTML: &str = include_str!("../assets/chat.html");
const UI_MODE_ENV: &str = "ZHONGSHU_ORB_UI";
const UI_DIST_ENV: &str = "ZHONGSHU_ORB_UI_DIST";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayAsset {
    React { index_path: PathBuf, html: String },
    LegacyHtml { reason: String },
}

pub fn legacy_chat_html() -> &'static str {
    LEGACY_CHAT_HTML
}

pub fn select_overlay_asset() -> OverlayAsset {
    let mode = env::var(UI_MODE_ENV).unwrap_or_else(|_| "auto".to_string());
    let explicit_dist = env::var_os(UI_DIST_ENV).map(PathBuf::from);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    select_overlay_asset_from(&mode, explicit_dist.as_deref(), &cwd, manifest_dir)
}

fn select_overlay_asset_from(
    mode: &str,
    explicit_dist: Option<&Path>,
    cwd: &Path,
    manifest_dir: &Path,
) -> OverlayAsset {
    let normalized_mode = mode.trim().to_ascii_lowercase();
    if normalized_mode == "legacy" {
        return OverlayAsset::LegacyHtml {
            reason: format!("{UI_MODE_ENV}=legacy"),
        };
    }

    let candidates = react_index_candidates(explicit_dist, cwd, manifest_dir);
    for candidate in candidates {
        if let Some(html) = load_react_html(&candidate) {
            return OverlayAsset::React {
                index_path: candidate,
                html,
            };
        }
    }

    let reason = if normalized_mode == "react" {
        format!("react UI requested but no built index.html was found; run pnpm --dir zhongshu-orb/ui build or set {UI_DIST_ENV}")
    } else {
        "react UI build output not found".to_string()
    };
    OverlayAsset::LegacyHtml { reason }
}

fn react_index_candidates(
    explicit_dist: Option<&Path>,
    cwd: &Path,
    manifest_dir: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(explicit) = explicit_dist {
        candidates.push(index_path(explicit));
    }
    candidates.push(manifest_dir.join("ui").join("dist").join("index.html"));
    candidates.push(
        cwd.join("zhongshu-orb")
            .join("ui")
            .join("dist")
            .join("index.html"),
    );
    candidates.push(cwd.join("ui").join("dist").join("index.html"));
    candidates
}

fn index_path(path: &Path) -> PathBuf {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("index.html"))
    {
        path.to_path_buf()
    } else {
        path.join("index.html")
    }
}

fn load_react_html(index_path: &Path) -> Option<String> {
    let mut html = fs::read_to_string(index_path).ok()?;
    let dist_dir = index_path.parent()?;
    html = inline_stylesheets(&html, dist_dir)?;
    html = inline_scripts(&html, dist_dir)?;
    Some(html)
}

fn inline_stylesheets(html: &str, dist_dir: &Path) -> Option<String> {
    replace_asset_tags(
        html,
        "href=\"",
        |asset_path, content| {
            if asset_path.ends_with(".css") {
                Some(format!("<style>\n{content}\n</style>"))
            } else {
                None
            }
        },
        dist_dir,
    )
}

fn inline_scripts(html: &str, dist_dir: &Path) -> Option<String> {
    replace_asset_tags(
        html,
        "src=\"",
        |asset_path, content| {
            if asset_path.ends_with(".js") {
                Some(format!("<script type=\"module\">\n{content}\n</script>"))
            } else {
                None
            }
        },
        dist_dir,
    )
}

fn replace_asset_tags(
    html: &str,
    attr: &str,
    replacement: impl Fn(&str, &str) -> Option<String>,
    dist_dir: &Path,
) -> Option<String> {
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(attr_start) = rest.find(attr) {
        let tag_start = rest[..attr_start].rfind('<')?;
        let path_start = attr_start + attr.len();
        let path_end = rest[path_start..].find('"')? + path_start;
        let asset_path = &rest[path_start..path_end];
        let tag_end = if attr == "src=\"" {
            rest[path_end..].find("</script>")? + path_end + "</script>".len()
        } else {
            rest[path_end..].find('>')? + path_end + 1
        };

        let Some(relative) = asset_path.strip_prefix("./") else {
            output.push_str(&rest[..tag_end]);
            rest = &rest[tag_end..];
            continue;
        };
        let content = fs::read_to_string(dist_dir.join(relative)).ok()?;
        let new_tag = replacement(asset_path, &content)?;
        output.push_str(&rest[..tag_start]);
        output.push_str(&new_tag);
        rest = &rest[tag_end..];
    }
    output.push_str(rest);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_mode_forces_legacy_html() {
        let selected = select_overlay_asset_from(
            "legacy",
            None,
            Path::new("D:/repo"),
            Path::new("D:/repo/zhongshu-orb"),
        );

        assert_eq!(
            selected,
            OverlayAsset::LegacyHtml {
                reason: "ZHONGSHU_ORB_UI=legacy".to_string()
            }
        );
    }

    #[test]
    fn explicit_dist_can_point_to_directory_or_index_file() {
        let root = unique_test_dir("dist-paths");
        let dist = root.join("ui-dist");
        write_react_dist(&dist);
        let selected =
            select_overlay_asset_from("auto", Some(&dist), &root, &root.join("zhongshu-orb"));

        assert!(matches!(
            selected,
            OverlayAsset::React { index_path, .. }
                if index_path == dist.join("index.html")
        ));

        let selected = select_overlay_asset_from(
            "auto",
            Some(&dist.join("index.html")),
            &root,
            &root.join("zhongshu-orb"),
        );

        assert!(matches!(selected, OverlayAsset::React { .. }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn auto_mode_falls_back_when_react_build_is_missing() {
        let selected = select_overlay_asset_from(
            "auto",
            None,
            Path::new("D:/repo"),
            Path::new("D:/repo/zhongshu-orb"),
        );

        assert!(matches!(selected, OverlayAsset::LegacyHtml { .. }));
    }

    #[test]
    fn react_dist_is_inlined_for_webview2_html_loading() {
        let root = unique_test_dir("inline");
        let dist = root.join("ui-dist");
        write_react_dist(&dist);

        let selected =
            select_overlay_asset_from("auto", Some(&dist), &root, &root.join("zhongshu-orb"));

        let OverlayAsset::React { html, .. } = selected else {
            panic!("expected react html");
        };

        assert!(html.contains("<style>"));
        assert!(html.contains("body{background:#000}"));
        assert!(html.contains("<script type=\"module\">"));
        assert!(html.contains("window.__ZHONGSHU_TEST__=true"));
        assert!(!html.contains("src=\"./assets/"));
        assert!(!html.contains("href=\"./assets/"));
        let _ = fs::remove_dir_all(root);
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "zhongshu-overlay-assets-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn write_react_dist(dist: &Path) {
        fs::create_dir_all(dist.join("assets")).unwrap();
        fs::write(
            dist.join("index.html"),
            r#"<!doctype html><html><head><script type="module" crossorigin src="./assets/index.js"></script><link rel="stylesheet" crossorigin href="./assets/index.css"></head><body><div id="root"></div></body></html>"#,
        )
        .unwrap();
        fs::write(
            dist.join("assets").join("index.js"),
            "window.__ZHONGSHU_TEST__=true;",
        )
        .unwrap();
        fs::write(
            dist.join("assets").join("index.css"),
            "body{background:#000}",
        )
        .unwrap();
    }
}

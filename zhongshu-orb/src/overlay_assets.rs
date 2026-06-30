use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};

const LEGACY_CHAT_HTML: &str = include_str!("../assets/chat.html");
const UI_MODE_ENV: &str = "ZHONGSHU_ORB_UI";
const UI_DIST_ENV: &str = "ZHONGSHU_ORB_UI_DIST";
const UI_LOADER_ENV: &str = "ZHONGSHU_ORB_UI_LOADER";
const PROTOCOL_NAME: &str = "zhongshu";
const PROTOCOL_URL: &str = "zhongshu://localhost/index.html";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayAsset {
    ReactProtocol {
        index_path: PathBuf,
        dist_dir: PathBuf,
    },
    ReactInline {
        index_path: PathBuf,
        html: String,
    },
    LegacyHtml {
        reason: String,
    },
}

pub fn legacy_chat_html() -> &'static str {
    LEGACY_CHAT_HTML
}

pub fn react_protocol_name() -> &'static str {
    PROTOCOL_NAME
}

pub fn react_protocol_url() -> &'static str {
    PROTOCOL_URL
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
    let loader = env::var(UI_LOADER_ENV).unwrap_or_else(|_| "protocol".to_string());
    select_overlay_asset_from_with_loader(mode, &loader, explicit_dist, cwd, manifest_dir)
}

fn select_overlay_asset_from_with_loader(
    mode: &str,
    loader: &str,
    explicit_dist: Option<&Path>,
    cwd: &Path,
    manifest_dir: &Path,
) -> OverlayAsset {
    let normalized_mode = mode.trim().to_ascii_lowercase();
    let loader = loader.trim().to_ascii_lowercase();
    if normalized_mode == "legacy" {
        return OverlayAsset::LegacyHtml {
            reason: format!("{UI_MODE_ENV}=legacy"),
        };
    }

    let candidates = react_index_candidates(explicit_dist, cwd, manifest_dir);
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        if loader == "inline" {
            if let Some(html) = load_react_html(&candidate) {
                return OverlayAsset::ReactInline {
                    index_path: candidate,
                    html,
                };
            }
            continue;
        }
        if let Some(dist_dir) = candidate.parent() {
            return OverlayAsset::ReactProtocol {
                index_path: candidate.clone(),
                dist_dir: dist_dir.to_path_buf(),
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

pub fn serve_react_protocol_asset(
    dist_dir: &Path,
    request: http::Request<Vec<u8>>,
) -> http::Response<Cow<'static, [u8]>> {
    let path = request.uri().path();
    let relative = if path == "/" || path.is_empty() {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    let relative_path = Path::new(relative);
    if relative_path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return protocol_response(
            http::StatusCode::FORBIDDEN,
            "text/plain; charset=utf-8",
            b"forbidden".to_vec(),
        );
    }

    let root = match fs::canonicalize(dist_dir) {
        Ok(root) => root,
        Err(e) => {
            return protocol_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                format!("cannot read UI dist: {e}").into_bytes(),
            );
        }
    };
    let candidate = match fs::canonicalize(root.join(relative_path)) {
        Ok(path) => path,
        Err(_) => {
            return protocol_response(
                http::StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                b"not found".to_vec(),
            );
        }
    };
    if !candidate.starts_with(&root) {
        return protocol_response(
            http::StatusCode::FORBIDDEN,
            "text/plain; charset=utf-8",
            b"forbidden".to_vec(),
        );
    }

    match fs::read(&candidate) {
        Ok(body) => protocol_response(http::StatusCode::OK, content_type(relative), body),
        Err(e) => protocol_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            format!("cannot read UI asset: {e}").into_bytes(),
        ),
    }
}

fn protocol_response(
    status: http::StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
) -> http::Response<Cow<'static, [u8]>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(
            http::header::CACHE_CONTROL,
            "no-cache, no-store, must-revalidate",
        )
        .header(http::header::EXPIRES, "0")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| http::Response::new(Cow::Borrowed(&b"response build failed"[..])))
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".wasm") {
        "application/wasm"
    } else {
        "application/octet-stream"
    }
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
            OverlayAsset::ReactProtocol { index_path, .. }
                if index_path == dist.join("index.html")
        ));

        let selected = select_overlay_asset_from(
            "auto",
            Some(&dist.join("index.html")),
            &root,
            &root.join("zhongshu-orb"),
        );

        assert!(matches!(selected, OverlayAsset::ReactProtocol { .. }));
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
    fn auto_mode_uses_protocol_loader_by_default() {
        let root = unique_test_dir("protocol");
        let dist = root.join("ui-dist");
        write_react_dist(&dist);

        let selected =
            select_overlay_asset_from("auto", Some(&dist), &root, &root.join("zhongshu-orb"));

        assert!(matches!(
            selected,
            OverlayAsset::ReactProtocol { index_path, dist_dir }
                if index_path == dist.join("index.html") && dist_dir == dist
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn protocol_loader_serves_dist_assets() {
        let root = unique_test_dir("protocol-serve");
        let dist = root.join("ui-dist");
        write_react_dist(&dist);

        let response = serve_react_protocol_asset(
            &dist,
            http::Request::builder()
                .uri("zhongshu://localhost/assets/index.js")
                .body(Vec::new())
                .unwrap(),
        );

        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert!(String::from_utf8_lossy(response.body()).contains("__ZHONGSHU_TEST__"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inline_loader_keeps_legacy_webview2_fallback() {
        let root = unique_test_dir("inline");
        let dist = root.join("ui-dist");
        write_react_dist(&dist);

        let selected = select_overlay_asset_from_with_loader(
            "auto",
            "inline",
            Some(&dist),
            &root,
            &root.join("zhongshu-orb"),
        );

        let OverlayAsset::ReactInline { html, .. } = selected else {
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

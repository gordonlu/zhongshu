pub fn format_evidence(content: &str, source: &str) -> String {
    format!("<evidence source=\"{}\">{}</evidence>", source, content)
}

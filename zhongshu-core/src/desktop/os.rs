pub fn open_file(path: &str) -> anyhow::Result<()> {
    open::that(path).map_err(|e| anyhow::anyhow!("os: failed to open '{path}': {e}"))
}

pub fn open_url(url: &str) -> anyhow::Result<()> {
    open::that(url).map_err(|e| anyhow::anyhow!("os: failed to open URL '{url}': {e}"))
}

pub fn open_in_browser(url: &str) -> anyhow::Result<()> {
    open::with(url, "firefox")
        .or_else(|_| open::with(url, "chrome"))
        .or_else(|_| open::that(url))
        .map_err(|e| anyhow::anyhow!("os: failed to open browser for '{url}': {e}"))
}

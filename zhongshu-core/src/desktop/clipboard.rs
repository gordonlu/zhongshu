pub fn read() -> anyhow::Result<String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: failed to open: {e}"))?;
    clipboard
        .get_text()
        .map_err(|e| anyhow::anyhow!("clipboard: failed to read: {e}"))
}

pub fn write(text: &str) -> anyhow::Result<()> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("clipboard: failed to open: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| anyhow::anyhow!("clipboard: failed to write: {e}"))
}

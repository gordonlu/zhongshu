use notify_rust::Notification;

pub fn show(title: &str, body: &str) -> anyhow::Result<()> {
    Notification::new()
        .appname("中书")
        .summary(title)
        .body(body)
        .show()
        .map_err(|e| anyhow::anyhow!("notification: failed to show: {e}"))?;
    Ok(())
}

pub fn show_urgent(title: &str, body: &str) -> anyhow::Result<()> {
    Notification::new()
        .appname("中书")
        .summary(title)
        .body(body)
        .urgency(notify_rust::Urgency::Critical)
        .show()
        .map_err(|e| anyhow::anyhow!("notification: failed to show urgent: {e}"))?;
    Ok(())
}

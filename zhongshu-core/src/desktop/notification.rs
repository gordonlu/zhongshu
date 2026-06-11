use std::sync::atomic::{AtomicBool, Ordering};
use notify_rust::Notification;

/// Global flag: user clicked an urgent notification (wants to focus overlay).
static FOCUS_OVERLAY: AtomicBool = AtomicBool::new(false);

pub fn show(title: &str, body: &str) -> anyhow::Result<()> {
    Notification::new()
        .appname("中书")
        .summary(title)
        .body(body)
        .show()
        .map_err(|e| anyhow::anyhow!("notification: failed to show: {e}"))?;
    Ok(())
}

/// Show an urgent notification.  When clicked, sets the global FOCUS_OVERLAY flag
/// so the main loop can focus the overlay window.
pub fn show_urgent(title: &str, body: &str) -> anyhow::Result<()> {
    Notification::new()
        .appname("中书")
        .summary(title)
        .body(body)
        .action("default", "打开")
        .urgency(notify_rust::Urgency::Critical)
        .show()
        .map(|handle| {
            std::thread::spawn(move || {
                handle.wait_for_action(|action| {
                    if action == "default" || action == "approve" {
                        FOCUS_OVERLAY.store(true, Ordering::Relaxed);
                    }
                });
            });
        })
        .map_err(|e| anyhow::anyhow!("notification: failed to show urgent: {e}"))?;
    Ok(())
}

/// Check and consume the focus-request flag.
pub fn consume_focus_request() -> bool {
    FOCUS_OVERLAY.swap(false, Ordering::Relaxed)
}

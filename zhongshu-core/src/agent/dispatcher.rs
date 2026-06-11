use crate::event::{Event, EventBus};

/// EventBus consumer that shows desktop notifications for attention events.
pub struct AttentionDispatcher {
    notify: Box<dyn Fn(&str, &str) + Send + Sync>,
}

impl AttentionDispatcher {
    pub fn new(notify: Box<dyn Fn(&str, &str) + Send + Sync>) -> Self {
        AttentionDispatcher { notify }
    }

    /// Subscribe to EventBus and notify on attention events.
    pub fn spawn(self, eb: &EventBus) -> tokio::task::JoinHandle<()> {
        let notify = self.notify;
        let mut rx = eb.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Event::Attention(attn)) => {
                        let report = match &attn {
                            crate::event::AttentionEvent::Interrupt { report } => report,
                            crate::event::AttentionEvent::Notify { report } => report,
                            crate::event::AttentionEvent::Digest { .. } => continue,
                        };
                        (notify)(&report.worker, &report.summary);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("attention dispatcher lagged: {n}");
                    }
                    Err(_) => break,
                }
            }
        })
    }
}

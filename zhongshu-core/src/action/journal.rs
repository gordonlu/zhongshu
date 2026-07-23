use crate::core::ledger::RunLedger;

pub struct ActionJournal {
    ledger: Option<RunLedger>,
    run_id: String,
}

impl ActionJournal {
    pub fn new(ledger: Option<RunLedger>, run_id: impl Into<String>) -> Self {
        Self {
            ledger,
            run_id: run_id.into(),
        }
    }

    pub fn record_start(&self, name: &str, args: &str, idempotency_key: &str) {
        if let Some(ref ledger) = self.ledger {
            let _ = ledger.record_tool_call(
                &self.run_id,
                name,
                args,
                "started",
                None,
                Some(idempotency_key),
            );
        }
    }

    pub fn record_completion(&self, name: &str, args: &str, idempotency_key: &str, status: &str) {
        if let Some(ref ledger) = self.ledger {
            let _ = ledger.record_tool_call(
                &self.run_id,
                name,
                args,
                status,
                None,
                Some(idempotency_key),
            );
        }
    }

    pub fn is_tool_completed(&self, idempotency_key: &str) -> bool {
        self.ledger
            .as_ref()
            .and_then(|l| l.is_tool_completed(&self.run_id, idempotency_key).ok())
            .unwrap_or(false)
    }

    pub fn reconcile_inflight(&self) -> Vec<(String, String, String)> {
        self.ledger
            .as_ref()
            .and_then(|l| l.reconcile_inflight_tools(&self.run_id).ok())
            .unwrap_or_default()
    }
}

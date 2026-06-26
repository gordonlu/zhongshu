use crate::harness::trace::event::HarnessEvent;

pub struct TraceLedger {
    pub events: Vec<HarnessEvent>,
}

impl TraceLedger {
    pub fn new() -> Self {
        TraceLedger { events: Vec::new() }
    }

    pub fn push(&mut self, event: HarnessEvent) {
        self.events.push(event);
    }

    pub fn all(&self) -> &[HarnessEvent] {
        &self.events
    }

    pub fn filter(&self, f: impl Fn(&HarnessEvent) -> bool) -> Vec<&HarnessEvent> {
        self.events.iter().filter(|e| f(e)).collect()
    }

    pub fn count(&self, f: impl Fn(&HarnessEvent) -> bool) -> usize {
        self.events.iter().filter(|e| f(e)).count()
    }

    pub fn to_jsonl(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_jsonl())
    }
}

use std::hash::Hash;
use std::path::PathBuf;

use crate::harness::state::{OpenViolation, ViolationKey, ViolationStatus};

/// Deduplication key for architecture violations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViolationDedupKey {
    pub rule_id: String,
    pub file_path: PathBuf,
    pub symbol_id: String,
}

impl From<&OpenViolation> for ViolationDedupKey {
    fn from(v: &OpenViolation) -> Self {
        ViolationDedupKey {
            rule_id: v.key.rule_id.clone(),
            file_path: v.key.file_path.clone(),
            symbol_id: v.key.symbol_id.clone(),
        }
    }
}

impl From<&ViolationKey> for ViolationDedupKey {
    fn from(key: &ViolationKey) -> Self {
        ViolationDedupKey {
            rule_id: key.rule_id.clone(),
            file_path: key.file_path.clone(),
            symbol_id: key.symbol_id.clone(),
        }
    }
}

impl From<ViolationKey> for ViolationDedupKey {
    fn from(key: ViolationKey) -> Self {
        ViolationDedupKey {
            rule_id: key.rule_id,
            file_path: key.file_path,
            symbol_id: key.symbol_id,
        }
    }
}

/// Deduplicate: skip if an Open/Acknowledged violation with same key exists.
pub fn dedup_violations(existing: &[OpenViolation], new_key: &ViolationDedupKey) -> bool {
    existing.iter().any(|v| {
        let k: ViolationDedupKey = v.into();
        &k == new_key
            && matches!(
                v.status,
                ViolationStatus::Open | ViolationStatus::Acknowledged
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::action::Severity;
    use crate::harness::state::ViolationKey;

    fn make_violation(rule: &str, file: &str, sym: &str) -> OpenViolation {
        OpenViolation {
            key: ViolationKey {
                rule_id: rule.into(),
                file_path: PathBuf::from(file),
                symbol_id: sym.into(),
            },
            status: ViolationStatus::Open,
            severity: Severity::Warning,
            confidence: crate::harness::action::Confidence::High,
            message: "test".into(),
            introduced_this_run: true,
            raised_step: 0,
        }
    }

    #[test]
    fn dedup_blocks_duplicate_key() {
        let existing = vec![make_violation("r1", "a.rs", "Foo")];
        let key = ViolationDedupKey {
            rule_id: "r1".into(),
            file_path: "a.rs".into(),
            symbol_id: "Foo".into(),
        };
        assert!(dedup_violations(&existing, &key));
    }

    #[test]
    fn dedup_allows_different_rule() {
        let existing = vec![make_violation("r1", "a.rs", "Foo")];
        let key = ViolationDedupKey {
            rule_id: "r2".into(),
            file_path: "a.rs".into(),
            symbol_id: "Foo".into(),
        };
        assert!(!dedup_violations(&existing, &key));
    }

    #[test]
    fn dedup_ignores_resolved() {
        let mut v = make_violation("r1", "a.rs", "Foo");
        v.status = ViolationStatus::Resolved;
        let existing = vec![v];
        let key = ViolationDedupKey {
            rule_id: "r1".into(),
            file_path: "a.rs".into(),
            symbol_id: "Foo".into(),
        };
        assert!(!dedup_violations(&existing, &key));
    }
}

//! Index-versus-filesystem verification.
//!
//! The index is a snapshot. When its rows drift from the filesystem, every
//! aggregate is quietly wrong and nothing in the response says so. That is how a
//! 9 GB undercount stayed invisible on a real machine: the index reported
//! 239 GB where the filesystem held 248 GB, and the only way to notice was to
//! compare against a different tool by hand.
//!
//! `search_status(verify_sample=N)` stats a sample of indexed paths and reports
//! the drift, so an agent can ask whether the numbers are trustworthy instead of
//! reasoning about it or guessing.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VerifyReport {
    pub sampled: usize,
    pub missing: usize,
    pub size_mismatch: usize,
    /// Up to five human-readable examples of drift.
    pub examples: Vec<String>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.missing == 0 && self.size_mismatch == 0
    }
}

/// Compare indexed `(path, size)` pairs against a stat function.
///
/// `stat` returns `None` when the path does not exist. It is a parameter so the
/// comparison can be tested without touching a real filesystem.
pub fn verify_entries<F>(entries: &[(String, u64)], stat: F) -> VerifyReport
where
    F: Fn(&str) -> Option<u64>,
{
    let mut report = VerifyReport {
        sampled: entries.len(),
        missing: 0,
        size_mismatch: 0,
        examples: Vec::new(),
    };
    for (path, indexed) in entries {
        match stat(path) {
            None => {
                report.missing += 1;
                if report.examples.len() < 5 {
                    report.examples.push(format!("{path} (missing on disk)"));
                }
            }
            Some(actual) if actual != *indexed => {
                report.size_mismatch += 1;
                if report.examples.len() < 5 {
                    report
                        .examples
                        .push(format!("{path} (indexed {indexed}, actual {actual})"));
                }
            }
            Some(_) => {}
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_sample_reports_no_drift() {
        let entries = vec![("C:/a".to_string(), 10), ("C:/b".to_string(), 20)];
        let r = verify_entries(&entries, |p| {
            if p == "C:/a" {
                Some(10)
            } else {
                Some(20)
            }
        });
        assert_eq!(r.sampled, 2);
        assert_eq!(r.missing, 0);
        assert_eq!(r.size_mismatch, 0);
        assert!(r.is_clean());
    }

    #[test]
    fn catches_missing_and_resized_entries() {
        let entries = vec![
            ("C:/gone".to_string(), 10),
            ("C:/grew".to_string(), 10),
            ("C:/ok".to_string(), 10),
        ];
        let r = verify_entries(&entries, |p| match p {
            "C:/gone" => None,
            "C:/grew" => Some(999),
            _ => Some(10),
        });
        assert_eq!(r.sampled, 3);
        assert_eq!(r.missing, 1);
        assert_eq!(r.size_mismatch, 1);
        assert!(!r.is_clean());
        assert!(r.examples.iter().any(|e| e.contains("C:/gone")));
        assert!(r.examples.iter().any(|e| e.contains("C:/grew")));
    }

    #[test]
    fn examples_are_capped_at_five() {
        let entries: Vec<(String, u64)> = (0..50).map(|i| (format!("C:/{i}"), 1)).collect();
        let r = verify_entries(&entries, |_| None);
        assert_eq!(r.missing, 50);
        assert_eq!(r.examples.len(), 5);
    }
}

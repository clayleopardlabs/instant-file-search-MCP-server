//! In-band query diagnostics.
//!
//! A file index that answers a malformed query with a confident empty result is
//! worse than one that errors. Every one of the observed agent mistakes
//! (`aggregate_files(query="*")`, a missing `include_all`, `OR` instead of `|`,
//! `path:` inside the query, an invalid filter token) returned a plausible,
//! wrong answer with no signal at all.
//!
//! This module turns those into something the model sees on the call it is
//! making: a `parsed` summary of what the server understood, a `warnings` array
//! with an exact fix, and a hard error for filter tokens that can only ever
//! match nothing.

use serde::Serialize;

/// Directories the indexer skips unless `include_all` is set.
///
/// Kept in step with `indexer/src/query.rs` DEFAULT_EXCLUDES. Under-reporting
/// this list is a correctness bug, not cosmetics: an agent told about only
/// three exclusions concludes that the recycle bin and System Volume
/// Information are empty, when both are indexed and reachable via include_all.
pub const DEFAULT_EXCLUDES: [&str; 5] = [
    "node_modules",
    ".git",
    "WinSxS",
    "$Recycle.Bin",
    "System Volume Information",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Warning {
    fn new(code: &'static str, message: impl Into<String>, fix: Option<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fix,
        }
    }
}

/// What the server understood from a request, echoed back with every response.
#[derive(Debug, Clone, Serialize)]
pub struct Parsed {
    /// The query exactly as received.
    pub query: String,
    /// True when the native engine will exclude directories.
    pub files_only: bool,
    /// True when `node_modules`/`.git`/`WinSxS` are being searched.
    pub include_all: bool,
    /// Active default exclusions; empty when `include_all` is set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excludes: Vec<&'static str>,
}

/// True when the query already chooses a file/folder type.
pub fn has_explicit_type(query: &str) -> bool {
    query.split_whitespace().any(|word| {
        let word = word.to_ascii_lowercase();
        word.starts_with("is:")
            || word.starts_with("file:")
            || word.starts_with("folder:")
            || word.starts_with("attrib:")
    })
}

fn tokens(query: &str) -> impl Iterator<Item = &str> {
    query.split_whitespace()
}

/// Filter tokens that cannot match anything, so a zero-result answer would be
/// misleading rather than informative.
///
/// Everything semantics treat these as "match nothing"; for an LLM-driven
/// server that is a trap, so the MCP layer rejects them instead.
pub fn invalid_filters(query: &str) -> Vec<String> {
    let mut bad = Vec::new();
    for raw in tokens(query) {
        let word = raw.trim_matches('"');
        let lower = word.to_ascii_lowercase();
        let bad_size = |value: &str| -> bool {
            let v = value.trim();
            if v.is_empty() {
                return true;
            }
            let without_op = v.trim_start_matches(['>', '<', '=']).trim();
            if without_op.is_empty() {
                return true;
            }
            // Units are optional and JEDEC-style: 10mb, 512kb, 2gb, 10b.
            let lower = without_op.to_ascii_lowercase();
            let digits = lower
                .strip_suffix("tb")
                .or_else(|| lower.strip_suffix("gb"))
                .or_else(|| lower.strip_suffix("mb"))
                .or_else(|| lower.strip_suffix("kb"))
                .or_else(|| lower.strip_suffix('b'))
                .unwrap_or(lower.as_str());
            digits.is_empty() || digits.parse::<f64>().is_err()
        };
        if let Some(v) = lower.strip_prefix("size:") {
            let value = &word[word.len() - v.len()..];
            // Named constants are valid (`size:large`, `size:empty`, ...).
            let named = matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "tiny" | "small" | "medium" | "large" | "huge" | "gigantic" | "empty"
            );
            if !named && bad_size(value) {
                bad.push(word.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("len:") {
            let value = &word[word.len() - v.len()..];
            if bad_size(value) {
                bad.push(word.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("frn:") {
            let value = &word[word.len() - v.len()..];
            if bad_size(value) {
                bad.push(word.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("ext:") {
            let value = &word[word.len() - v.len()..];
            if value.trim().is_empty() {
                bad.push(word.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("attrib:") {
            let value = &word[word.len() - v.len()..];
            let value = value.trim_start_matches('!');
            let known = "arsdntpcoeihgmu";
            if value.is_empty()
                || !value
                    .chars()
                    .all(|c| known.contains(c.to_ascii_lowercase()))
            {
                bad.push(word.to_string());
            }
        }
    }
    bad
}

/// Classify a filter token so scoping problems can be detected by kind rather
/// than by exact text (`file:` and `is:file` are the same kind).
fn filter_kind(token: &str) -> Option<&'static str> {
    let t = token.trim_matches('"').to_ascii_lowercase();
    let t = t.trim_start_matches('!');
    const KINDS: [(&str, &str); 12] = [
        ("is:", "type"),
        ("file:", "type"),
        ("folder:", "type"),
        ("attrib:", "attrib"),
        ("size:", "size"),
        ("len:", "len"),
        ("frn:", "frn"),
        ("ext:", "ext"),
        ("dm:", "date"),
        ("dc:", "date"),
        ("da:", "date"),
        ("content:", "content"),
    ];
    KINDS
        .iter()
        .find(|(p, _)| t.starts_with(p))
        .map(|(_, k)| *k)
}

/// Warnings for a query, given how it will actually be executed.
pub fn analyze(query: &str, include_all: bool) -> Vec<Warning> {
    let mut warnings = Vec::new();

    // `OR` is not an operator here; the pipe is. An uppercase OR silently
    // becomes a literal name term and the query returns nothing.
    for raw in tokens(query) {
        if raw.eq_ignore_ascii_case("or") && raw == "OR" {
            let fixed = query
                .split_whitespace()
                .map(|w| if w == "OR" { "|" } else { w })
                .collect::<Vec<_>>()
                .join(" ");
            warnings.push(Warning::new(
                "SUSPECT_OR",
                "'OR' was treated as a literal file-name term, not as a boolean operator, so this query can only match a file literally named OR.",
                Some(fixed),
            ));
            break;
        }
    }

    // Folder scope belongs in the `path` parameter. Inside the query it is a
    // path-prefix match that fails whenever the path contains a space.
    for raw in tokens(query) {
        if raw.to_ascii_lowercase().starts_with("path:") {
            let scope = raw
                .trim_matches('"')
                .split_once(':')
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            warnings.push(Warning::new(
                "PATH_IN_QUERY",
                "path: inside the query is a path-prefix match and fails for paths containing spaces. Use the `path` parameter for folder scope.",
                Some(format!("path=\"{scope}\"")),
            ));
            break;
        }
    }

    // `|` splits the query into alternatives, and a filter binds only to the
    // alternative it appears in. `*.bin | *.7z is:folder` means
    // "(*.bin) OR (*.7z AND folder)": not "(*.bin OR *.7z) AND folder", which
    // is what a caller means. The unfiltered side also matches directories, so
    // a count can come back higher than the file count for no visible reason.
    let groups: Vec<&str> = query
        .split('|')
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect();
    if groups.len() > 1 {
        let per_group: Vec<Vec<(&'static str, String)>> = groups
            .iter()
            .map(|g| {
                g.split_whitespace()
                    .filter_map(|w| filter_kind(w).map(|k| (k, w.trim_matches('"').to_string())))
                    .collect()
            })
            .collect();
        let mut kinds: Vec<&'static str> = Vec::new();
        for (kind, _) in per_group.iter().flatten() {
            if !kinds.contains(kind) {
                kinds.push(kind);
            }
        }
        for kind in kinds {
            if per_group
                .iter()
                .all(|g| g.iter().any(|(k, _)| *k == kind))
            {
                continue;
            }
            let token = per_group
                .iter()
                .flatten()
                .find(|(k, _)| *k == kind)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            let fixed = groups
                .iter()
                .map(|g| {
                    if g.split_whitespace().any(|w| filter_kind(w) == Some(kind)) {
                        g.to_string()
                    } else {
                        format!("{g} {token}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" | ");
            warnings.push(Warning::new(
                "PIPE_FILTER_SCOPE",
                format!(
                    "'{token}' applies only to the alternative it sits in. The pipe splits this into separate groups, so the other side is unfiltered and can match files or directories you meant to exclude."
                ),
                Some(fixed),
            ));
        }
    }

    // Invalid filters match nothing, so a zero result would look like "absent".
    for bad in invalid_filters(query) {
        warnings.push(Warning::new(
            "INVALID_FILTER",
            format!("'{bad}' is not a valid filter and matches nothing."),
            None,
        ));
    }

    if !include_all {
        warnings.push(Warning::new(
            "DEFAULT_EXCLUDES_ACTIVE",
            "node_modules, .git, WinSxS, $Recycle.Bin and System Volume Information are excluded from these results. Pass include_all=true for a true total.",
            Some("include_all=true".to_string()),
        ));
    }

    // No folder-inflation warning is needed any more: aggregate_files applies
    // the same files-by-default rule as find_files and count_files, so a bare
    // `*` cannot pull folder rows into the size total in the first place.
    warnings
}

/// The `parsed` block echoed with every tool response.
pub fn parsed(query: &str, include_all: bool) -> Parsed {
    Parsed {
        query: query.to_string(),
        files_only: !has_explicit_type(query),
        include_all,
        excludes: if include_all {
            Vec::new()
        } else {
            DEFAULT_EXCLUDES.to_vec()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(w: &[Warning]) -> Vec<&'static str> {
        w.iter().map(|x| x.code).collect()
    }

    #[test]
    fn detects_uppercase_or() {
        let w = analyze("*.a OR *.b", true);
        assert!(codes(&w).contains(&"SUSPECT_OR"));
        assert_eq!(w[0].fix.as_deref(), Some("*.a | *.b"));
        // The pipe is the operator, so it must not warn.
        assert!(!codes(&analyze("*.a | *.b", true)).contains(&"SUSPECT_OR"));
        // A lowercase `or` is a legitimate name substring; leave it alone.
        assert!(!codes(&analyze("or", true)).contains(&"SUSPECT_OR"));
    }

    #[test]
    fn detects_path_in_query() {
        let w = analyze("*.sav path:C:/Users/a b/x", true);
        assert!(codes(&w).contains(&"PATH_IN_QUERY"));
        assert!(!codes(&analyze("*.sav", true)).contains(&"PATH_IN_QUERY"));
    }

    #[test]
    fn warns_when_a_filter_scopes_only_one_alternative() {
        // Regression from a field test: an agent wrote `*.bin | *.7z is:folder`
        // expecting a global folder filter. The pipe splits alternatives, so the
        // first group was unfiltered and directories named *.bin came through.
        let w = analyze("*.bin | *.7z is:folder", true);
        let pipe = w
            .iter()
            .find(|x| x.code == "PIPE_FILTER_SCOPE")
            .expect("expected a PIPE_FILTER_SCOPE warning");
        assert_eq!(
            pipe.fix.as_deref(),
            Some("*.bin is:folder | *.7z is:folder")
        );
        // Repeating the filter on both sides is correct and must stay quiet.
        assert!(!codes(&analyze("*.bin file: | *.7z file:", true))
            .contains(&"PIPE_FILTER_SCOPE"));
        // No filters means nothing to scope.
        assert!(!codes(&analyze("*.a | *.b", true)).contains(&"PIPE_FILTER_SCOPE"));
        // A single alternative is not a scoping problem.
        assert!(!codes(&analyze("*.bin is:folder", true)).contains(&"PIPE_FILTER_SCOPE"));
    }

    #[test]
    fn flags_default_excludes_only_when_active() {
        assert!(codes(&analyze("*.rs", false)).contains(&"DEFAULT_EXCLUDES_ACTIVE"));
        assert!(!codes(&analyze("*.rs", true)).contains(&"DEFAULT_EXCLUDES_ACTIVE"));
    }

    #[test]
    fn rejects_invalid_filters() {
        assert_eq!(invalid_filters("size:garbage"), vec!["size:garbage"]);
        assert_eq!(invalid_filters("ext:"), vec!["ext:"]);
        assert_eq!(invalid_filters("attrib:q"), vec!["attrib:q"]);
        assert_eq!(invalid_filters("len:>"), vec!["len:>"]);
        // Valid forms stay quiet.
        assert!(invalid_filters("size:>10mb").is_empty());
        assert!(invalid_filters("size:large").is_empty());
        assert!(invalid_filters("ext:rs,py").is_empty());
        assert!(invalid_filters("attrib:h").is_empty());
        assert!(invalid_filters("attrib:!d").is_empty());
    }

    #[test]
    fn parsed_reports_files_only_and_excludes() {
        let p = parsed("*.rs", false);
        assert!(p.files_only);
        assert_eq!(p.excludes, DEFAULT_EXCLUDES.to_vec());
        // An explicit type opts out of files_only.
        assert!(!parsed("is:folder", false).files_only);
        // include_all clears the exclusion list.
        assert!(parsed("*.rs", true).excludes.is_empty());
    }
}

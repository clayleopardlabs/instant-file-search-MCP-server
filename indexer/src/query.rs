//! Query engine — Everything-compatible search over the in-memory index.
//!
//! Supports the same query syntax surface the MCP tools expose:
//! wildcards (`*`, `?`), `regex:`, `case:`, `dm:`, `dc:`, `da:`, `size:`,
//! `ext:`, `path:`, `folder:`, `file:`, `!` NOT, `|` OR, `<...>` grouping.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::mft::IndexedFile;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryOptions {
    pub query: String,
    pub path: Option<String>,
    pub exclude_path: Option<String>,
    pub include_all: bool,
    pub match_path: bool,
    pub regex: bool,
    pub match_case: bool,
    pub match_whole_word: bool,
    pub max_results: usize,
    pub offset: usize,
    pub sort: Option<String>,
}

#[derive(Debug, Default)]
pub struct QueryResult {
    pub total: usize,
    pub entries: Vec<IndexedFile>,
}

#[derive(Debug, Clone)]
pub enum Token {
    /// Positive term (name wildcard or bare text).
    Include { pattern: String, whole_word: bool },
    /// Negative term (`!pattern`).
    Exclude { pattern: String, whole_word: bool },
    /// Regex term (`regex:...` or regex flag).
    Regex { pattern: String, negate: bool },
    /// Or-group: list of terms where any must match.
    Or(Vec<Token>),
    /// `path:` scoping.
    Path(String),
    /// `ext:` extension filter (one or more, comma separated).
    Ext(Vec<String>),
    /// `size:` filter: `>N`, `<N`, `N..M`, or plain `N` (bytes; K/M/G suffixes).
    Size(SizeFilter),
    /// `dm:`, `dc:`, `da:` date filter (unix seconds).
    Date { kind: DateKind, op: DateOp, value: i64 },
    /// `folder:` / `file:` type filter.
    TypeFilter(TypeFilter),
    /// Plain scoping term like `C:\foo` (implicit path).
    BarePath(String),
}

#[derive(Debug, Clone)]
pub enum DateKind {
    Modified,
    Created,
    Accessed,
}

#[derive(Debug, Clone)]
pub enum DateOp {
    Before,
    After,
    On,
    Range { end: i64 },
}

#[derive(Debug, Clone)]
pub enum SizeFilter {
    Greater(u64),
    Less(u64),
    Equal(u64),
    Range { min: u64, max: u64 },
}

#[derive(Debug, Clone, Copy)]
pub enum TypeFilter {
    Files,
    Folders,
}

/// Default directories excluded from searches unless `include_all`.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules",
    ".git",
    "WinSxS",
    "$Recycle.Bin",
    "System Volume Information",
];

/// Parse a size string like `10mb`, `1kb`, `>5gb`, `1kb..1mb`.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("kb") {
        (n, 1000u64)
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, 1000 * 1000)
    } else if let Some(n) = s.strip_suffix("gb") {
        (n, 1000 * 1000 * 1000)
    } else if let Some(n) = s.strip_suffix("kib") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix("mib") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("gib") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 1)
    } else {
        (s.as_str(), 1)
    };
    let val: f64 = num.trim().parse().ok()?;
    Some((val * mult as f64) as u64)
}

/// Parse a date token: `today`, `yesterday`, `Ndays`, or an ISO date.
fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim().to_ascii_lowercase();
    let now = chrono_now();
    if s == "today" {
        return Some(start_of_day(now));
    }
    if s == "yesterday" {
        return Some(start_of_day(now - 86400));
    }
    if let Some(n) = s.strip_suffix("days") {
        if let Ok(n) = n.trim().parse::<i64>() {
            return Some(start_of_day(now - n * 86400));
        }
    }
    // ISO-8601-ish: YYYY-MM-DD or YYYY-MM-DD HH:MM:SS
    let s2 = s.replace('T', " ");
    if s2.len() >= 10 && s2.as_bytes()[4] == b'-' && s2.as_bytes()[7] == b'-' {
        let year: i64 = s2[0..4].parse().ok()?;
        let month: i64 = s2[5..7].parse().ok()?;
        let day: i64 = s2[8..10].parse().ok()?;
        let mut days = 0i64;
        for y in 1970..year {
            days += if is_leap(y) { 366 } else { 365 };
        }
        for m in 1..month {
            days += days_in_month(year, m);
        }
        days += day - 1;
        return Some(days * 86400);
    }
    None
}

fn chrono_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn start_of_day(unix: i64) -> i64 {
    unix - (unix % 86400)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Tokenize a query string into search tokens.
pub fn tokenize(query: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut group: Vec<Token> = Vec::new();

    let flush_group = |tokens: &mut Vec<Token>, group: &mut Vec<Token>| {
        if group.len() == 1 {
            tokens.push(group.pop().unwrap());
        } else if group.len() > 1 {
            tokens.push(Token::Or(std::mem::take(group)));
        }
        group.clear();
    };

    let parts = split_query(query);
    for raw in parts {
        if raw == "|" {
            flush_group(&mut tokens, &mut group);
            continue;
        }
        let negate = raw.starts_with('!') && !raw.starts_with("!<");
        let term = if negate { &raw[1..] } else { raw.as_str() };
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let lower = term.to_ascii_lowercase();

        let token = if term.starts_with("regex:") {
            Token::Regex { pattern: term[6..].trim().to_string(), negate }
        } else if term.starts_with("case:") {
            // `case:term` — case-sensitive term.
            let pat = term[5..].trim().to_string();
            if negate {
                Token::Exclude { pattern: pat, whole_word: false }
            } else {
                Token::Include { pattern: pat, whole_word: false }
            }
        } else if term.starts_with("path:") {
            Token::Path(term[5..].trim().trim_matches('"').to_string())
        } else if term.starts_with("ext:") {
            let exts: Vec<String> = term[4..]
                .split(',')
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            if !exts.is_empty() {
                Token::Ext(exts)
            } else {
                continue;
            }
        } else if term.starts_with("folder:") {
            Token::TypeFilter(TypeFilter::Folders)
        } else if term.starts_with("file:") {
            Token::TypeFilter(TypeFilter::Files)
        } else if lower.starts_with("size:") {
            match parse_size_filter(&term[5..]) {
                Some(f) => Token::Size(f),
                None => continue,
            }
        } else if lower.starts_with("dm:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Modified, op, value: v },
                None => continue,
            }
        } else if lower.starts_with("dc:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Created, op, value: v },
                None => continue,
            }
        } else if lower.starts_with("da:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Accessed, op, value: v },
                None => continue,
            }
        } else if term.starts_with("!<") && term.ends_with('>') {
            Token::Exclude { pattern: term[2..term.len() - 1].to_string(), whole_word: false }
        } else if is_windows_path(term) {
            Token::BarePath(term.to_string())
        } else if negate {
            Token::Exclude { pattern: term.to_string(), whole_word: false }
        } else {
            Token::Include { pattern: term.to_string(), whole_word: false }
        };
        group.push(token);
    }
    flush_group(&mut tokens, &mut group);
    tokens
}

/// Split a query on whitespace but keep `|` as its own token. Quoted strings
/// are kept together.
fn split_query(query: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '|' if !in_quote => {
                if !cur.trim().is_empty() {
                    parts.push(cur.trim().to_string());
                }
                parts.push("|".to_string());
                cur.clear();
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.trim().is_empty() {
                    parts.push(cur.trim().to_string());
                }
                cur.clear();
            }
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

fn is_windows_path(s: &str) -> bool {
    s.len() >= 3 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[2] == b'\\'
}

fn parse_size_filter(s: &str) -> Option<SizeFilter> {
    let s = s.trim();
    if let Some(range) = s.split_once("..") {
        return Some(SizeFilter::Range {
            min: parse_size(range.0)?,
            max: parse_size(range.1)?,
        });
    }
    if let Some(v) = s.strip_prefix('>') {
        return Some(SizeFilter::Greater(parse_size(v)?));
    }
    if let Some(v) = s.strip_prefix('<') {
        return Some(SizeFilter::Less(parse_size(v)?));
    }
    if let Some(v) = s.strip_prefix('=') {
        return Some(SizeFilter::Equal(parse_size(v)?));
    }
    Some(SizeFilter::Equal(parse_size(s)?))
}

fn parse_date_filter(s: &str) -> Option<(DateOp, i64)> {
    let s = s.trim();
    if let Some(v) = s.strip_prefix('>') {
        return Some((DateOp::After, parse_date(v)?));
    }
    if let Some(v) = s.strip_prefix('<') {
        return Some((DateOp::Before, parse_date(v)?));
    }
    if let Some(r) = s.split_once("..") {
        return Some((
            DateOp::Range { end: parse_date(r.1)? },
            parse_date(r.0)?,
        ));
    }
    Some((DateOp::On, parse_date(s)?))
}

fn eq_ci(a: u8, b: u8) -> bool {
    a == b || a.to_ascii_lowercase() == b.to_ascii_lowercase()
}

/// Everything semantics: a bare term (no wildcards) matches as a
/// case-insensitive substring anywhere in the name.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.iter().zip(n).all(|(a, b)| eq_ci(*a, *b)))
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    let s = s.as_bytes();
    let p = prefix.as_bytes();
    s.len() >= p.len() && s[..p.len()].iter().zip(p).all(|(a, b)| eq_ci(*a, *b))
}

/// Everything-style wildcard match: `*` = any run, `?` = one char.
/// Case-insensitive by default (byte-wise, no allocation).
pub fn wildcard_match(pattern: &str, text: &str, match_case: bool) -> bool {
    if !pattern.contains(['*', '?']) {
        return if match_case {
            text.contains(pattern)
        } else {
            contains_ci(text, pattern)
        };
    }
    if match_case {
        wildcard_rec(pattern.as_bytes(), text.as_bytes())
    } else {
        wildcard_rec_ci(pattern.as_bytes(), text.as_bytes())
    }
}

fn wildcard_rec(p: &[u8], t: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star_p = pi;
            star_t = ti;
            pi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn wildcard_rec_ci(p: &[u8], t: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || eq_ci(p[pi], t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star_p = pi;
            star_t = ti;
            pi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn whole_word_match(text: &str, needle: &str, match_case: bool) -> bool {
    let t = text.as_bytes();
    let n = needle.as_bytes();
    let words: Vec<&[u8]> = t
        .split(|c| !c.is_ascii_alphanumeric() && *c != b'_' && *c != b'.' && *c != b'-')
        .collect();
    words.iter().any(|w| {
        *w == n
            || (!match_case && w.len() == n.len() && w.iter().zip(n).all(|(a, b)| eq_ci(*a, *b)))
    })
}

fn file_matches(
    entry: &IndexedFile,
    tokens: &[Token],
    compiled: &HashMap<String, (regex::Regex, bool)>,
    opts: &QueryOptions,
) -> bool {
    let name = entry.name.as_str();
    let lower_path = entry.lower_path.as_str();
    let target = if opts.match_path { lower_path } else { name };

    for token in tokens {
        let ok = match token {
            Token::Include { pattern, whole_word } => {
                if *whole_word {
                    whole_word_match(target, pattern, opts.match_case)
                } else {
                    wildcard_match(pattern, target, opts.match_case)
                }
            }
            Token::Exclude { pattern, whole_word } => {
                // Match against the whole path for `!<dir>` and against the
                // name for plain `!term`.
                let hit = if pattern.contains('\\') || pattern.contains('/') {
                    if opts.match_case {
                        entry.path.contains(pattern.as_str())
                    } else {
                        contains_ci(&entry.path, pattern)
                    }
                } else if *whole_word {
                    whole_word_match(target, pattern, opts.match_case)
                } else {
                    wildcard_match(pattern, target, opts.match_case)
                };
                !hit
            }
            Token::Regex { pattern, negate } => {
                let hit = compiled
                    .get(pattern)
                    .map(|(re, _)| re.is_match(target))
                    .unwrap_or(false);
                if *negate {
                    !hit
                } else {
                    hit
                }
            }
            Token::Or(group) => group.iter().any(|t| file_matches(entry, std::slice::from_ref(t), compiled, opts)),
            Token::Path(p) | Token::BarePath(p) => {
                let p = p.trim_end_matches('\\');
                let mut ok = starts_with_ci(lower_path, p);
                if ok && lower_path.len() > p.len() {
                    ok = lower_path.as_bytes()[p.len()] == b'\\' || lower_path.as_bytes()[p.len()] == b':';
                }
                ok
            }
            Token::Ext(exts) => {
                let ext = entry.extension.as_deref().unwrap_or_default();
                exts.iter().any(|e| e == &ext)
            }
            Token::Size(f) => size_match(f, entry.size),
            Token::Date { kind, op, value } => {
                let ts = match kind {
                    DateKind::Modified => entry.modified,
                    DateKind::Created => entry.created,
                    DateKind::Accessed => entry.accessed,
                };
                let unix = windows_ts_to_unix(ts);
                date_match(op, *value, unix)
            }
            Token::TypeFilter(tf) => match tf {
                TypeFilter::Files => !entry.is_dir,
                TypeFilter::Folders => entry.is_dir,
            },
        };
        if !ok {
            return false;
        }
    }
    true
}

fn size_match(f: &SizeFilter, size: u64) -> bool {
    match f {
        SizeFilter::Greater(v) => size > *v,
        SizeFilter::Less(v) => size < *v,
        SizeFilter::Equal(v) => size == *v,
        SizeFilter::Range { min, max } => size >= *min && size <= *max,
    }
}

fn date_match(op: &DateOp, value: i64, ts: i64) -> bool {
    match op {
        DateOp::Before => ts < value,
        DateOp::After => ts >= value + 86400,
        DateOp::On => ts >= value && ts < value + 86400,
        DateOp::Range { end } => ts >= value && ts < *end,
    }
}

/// NTFS 100ns intervals since 1601 -> unix seconds.
fn windows_ts_to_unix(ts: i64) -> i64 {
    const EPOCH: i64 = 116_444_736_000_000_000;
    if ts < EPOCH {
        return 0;
    }
    ((ts - EPOCH) / 10_000_000) as i64
}

/// Run a search against a snapshot.
pub fn search(entries: &HashMap<String, IndexedFile>, opts: &QueryOptions) -> QueryResult {
    let tokens = tokenize(&opts.query);
    let scope = opts.path.as_deref().unwrap_or("").trim_end_matches('\\');
    let scope_lower = scope.to_ascii_lowercase();
    let exclude_parts: Vec<String> = opts
        .exclude_path
        .as_deref()
        .unwrap_or("")
        .split(';')
        .map(|p| p.trim().trim_end_matches('\\').to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    let mut compiled: HashMap<String, (regex::Regex, bool)> = HashMap::new();
    for t in &tokens {
        if let Token::Regex { pattern, negate } = t {
            if let Ok(re) = regex::RegexBuilder::new(pattern)
                .case_insensitive(!opts.match_case)
                .build()
            {
                compiled.insert(pattern.clone(), (re, *negate));
            }
        }
    }

    let mut matched: Vec<&IndexedFile> = Vec::new();
    for entry in entries.values() {
        if !opts.include_all && entry.excluded {
            continue;
        }
        if !scope_lower.is_empty() {
            let lp = entry.lower_path.as_str();
            if !starts_with_ci(lp, &scope_lower) {
                continue;
            }
        }
        if !exclude_parts.is_empty() {
            let lp = entry.lower_path.as_str();
            let mut excluded = false;
            for p in &exclude_parts {
                if starts_with_ci(lp, p) && (lp.len() == p.len() || lp.as_bytes()[p.len()] == b'\\') {
                    excluded = true;
                    break;
                }
            }
            if excluded {
                continue;
            }
        }
        if !opts.query.is_empty() && !file_matches(entry, &tokens, &compiled, opts) {
            continue;
        }
        matched.push(entry);
    }

    // Sort: default by name (case-insensitive), then full path.
    // For large result sets only the first max_results (+offset) are ever
    // returned, so use partial selection instead of a full sort.
    let want = opts.offset.saturating_add(if opts.max_results == 0 { usize::MAX } else { opts.max_results });
    let sort_key: Box<dyn Fn(&&IndexedFile, &&IndexedFile) -> std::cmp::Ordering> =
        match opts.sort.as_deref() {
            Some("name") | None => Box::new(|a, b| {
                cmp_ci(a.name.as_bytes(), b.name.as_bytes())
                    .then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("name_desc") => Box::new(|a, b| {
                cmp_ci(b.name.as_bytes(), a.name.as_bytes())
                    .then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("path") => Box::new(|a, b| cmp_ci(a.path.as_bytes(), b.path.as_bytes())),
            Some("path_desc") => Box::new(|a, b| cmp_ci(b.path.as_bytes(), a.path.as_bytes())),
            Some("size") => Box::new(|a, b| {
                b.size.cmp(&a.size).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("size_asc") => Box::new(|a, b| {
                a.size.cmp(&b.size).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_modified") => Box::new(|a, b| {
                b.modified.cmp(&a.modified).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_modified_asc") => Box::new(|a, b| {
                a.modified.cmp(&b.modified).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_created") => Box::new(|a, b| {
                b.created.cmp(&a.created).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_created_asc") => Box::new(|a, b| {
                a.created.cmp(&b.created).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_accessed") => Box::new(|a, b| {
                b.accessed.cmp(&a.accessed).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("date_accessed_asc") => Box::new(|a, b| {
                a.accessed.cmp(&b.accessed).then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
            Some("extension") => Box::new(|a, b| {
                a.extension
                    .cmp(&b.extension)
                    .then_with(|| cmp_ci(a.name.as_bytes(), b.name.as_bytes()))
            }),
            Some("extension_desc") => Box::new(|a, b| {
                b.extension
                    .cmp(&a.extension)
                    .then_with(|| cmp_ci(a.name.as_bytes(), b.name.as_bytes()))
            }),
            _ => Box::new(|a, b| {
                cmp_ci(a.name.as_bytes(), b.name.as_bytes())
                    .then_with(|| cmp_ci(a.path.as_bytes(), b.path.as_bytes()))
            }),
        };
    if matched.len() > want && want != usize::MAX {
        matched.select_nth_unstable_by(want, &sort_key);
        matched[..want].sort_by(&sort_key);
    } else {
        matched.sort_by(&sort_key);
    }

    let total = matched.len();
    let slice = matched
        .iter()
        .skip(opts.offset)
        .take(if opts.max_results == 0 { usize::MAX } else { opts.max_results })
        .map(|e| (*e).clone())
        .collect();

    QueryResult { total, entries: slice }
}

fn cmp_ci(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let x = a[i].to_ascii_lowercase();
        let y = b[i].to_ascii_lowercase();
        if x != y {
            return x.cmp(&y);
        }
    }
    a.len().cmp(&b.len())
}

//! Query engine — Everything-compatible search over the in-memory index.
//!
//! Supports the same query syntax surface the MCP tools expose:
//! wildcards (`*`, `?`), `regex:`, `case:`, `dm:`, `dc:`, `da:`, `size:`,
//! `ext:`, `path:`, `folder:`, `file:`, `!` NOT, `|` OR, `<...>` grouping.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::IndexedFile;

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
    /// Lowercase paths allowed by a `content:"..."` constraint. When set,
    /// only entries whose lowercased path appears here pass the filter.
    /// Populated by the pipe layer from the ContentStore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_paths: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct QueryResult {
    pub total: usize,
    pub entries: Vec<IndexedFile>,
}

/// Aggregation options for the `aggregate` query method.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AggregateOptions {
    pub query: String,
    pub path: Option<String>,
    pub exclude_path: Option<String>,
    pub include_all: bool,
    pub match_path: bool,
    pub regex: bool,
    pub match_case: bool,
    pub match_whole_word: bool,
    /// How many of the largest entries to return (default 20).
    pub top: usize,
    /// Lowercase paths allowed by a `content:"..."` constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_paths: Option<Vec<String>>,
}

/// Result of an aggregation query.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AggregateResult {
    /// Number of matched entries (files + folders).
    pub total: usize,
    /// Matched file count.
    pub files: usize,
    /// Matched folder count.
    pub folders: usize,
    /// Sum of sizes over all matched entries (files' own size; folders'
    /// recursive tree-summed size).
    pub total_size: u64,
    /// The `top` largest matched entries by size.
    pub largest: Vec<AggregateLargest>,
    /// Per-extension counts and size totals over matched files.
    pub by_extension: Vec<AggregateExt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateLargest {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateExt {
    pub extension: String,
    pub count: usize,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub enum Token {
    /// Positive term (name wildcard or bare text).
    Include { pattern: String, whole_word: bool, case_sensitive: bool, anchored_start: bool, anchored_end: bool },
    /// Negative term (`!pattern`).
    Exclude { pattern: String, whole_word: bool, case_sensitive: bool, anchored_start: bool, anchored_end: bool },
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
    /// `len:` filename-length filter (same operators as size:).
    Len(SizeFilter),
    /// `frn:` file-reference-number filter (same operators as size:).
    Frn(SizeFilter),
    /// `dm:`, `dc:`, `da:` date filter (unix seconds).
    Date { kind: DateKind, op: DateOp, value: i64 },
    /// `folder:` / `file:` type filter.
    TypeFilter(TypeFilter),
    /// `attrib:` NTFS attribute filter (mask of required FILE_ATTRIBUTE bits).
    Attrib { mask: u32, negate: bool },
    /// Matches nothing (unknown attribute letters).
    Never,
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
    Span { start: i64, end: i64 },
}

#[derive(Debug, Clone)]
pub enum SizeFilter {
    Greater(u64),
    GreaterOrEqual(u64),
    Less(u64),
    LessOrEqual(u64),
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
/// Everything's default size standard is JEDEC: kb=1024, mb=1024^2, gb=1024^3
/// (the `metric:` modifier opts into decimal 1000-based units).
/// Returns the byte value and the unit multiplier (>1 when a unit suffix was
/// present). Everything treats a bare unit value as a range up to the next
/// unit (`size:1kb` = 1024..2048), so the caller needs to know the multiplier.
fn parse_size_parts(s: &str, metric: bool) -> Option<(u64, u64)> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("kb") {
        (n, if metric { 1000u64 } else { 1024 })
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, if metric { 1000 * 1000 } else { 1024 * 1024 })
    } else if let Some(n) = s.strip_suffix("gb") {
        (n, if metric { 1000 * 1000 * 1000 } else { 1024 * 1024 * 1024 })
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
    Some(((val * mult as f64) as u64, mult))
}

fn parse_size(s: &str) -> Option<u64> {
    parse_size_parts(s, false).map(|(v, _)| v)
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
        // Local midnight of the date, in wall-clock-unix scale. The entry side
        // (query.rs) compares ts as `windows_ts_to_unix + local_offset`, so the
        // offset must NOT be added here or the two cancel and absolute dates
        // shift to UTC midnights (Everything uses local midnights).
        return Some(days * 86400);
    }
    None
}

fn chrono_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    utc + local_offset_secs()
}

/// Seconds east of UTC for the local timezone, resolved once per process.
/// Everything evaluates relative dates (today/yesterday/last7days/...) in
/// local time, so our boundaries must too. Cached because the syscall is not
/// cheap enough for per-query use.
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    use windows::Win32::System::SystemInformation::{
        GetLocalTime, GetSystemTimeAsFileTime,
    };
    static CACHE: OnceLock<i64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let lt = unsafe { GetLocalTime() };
        let ft = unsafe { GetSystemTimeAsFileTime() };
        let utc_unix = {
            const EPOCH: i64 = 116_444_736_000_000_000;
            let raw = (ft.dwHighDateTime as i64) << 32 | ft.dwLowDateTime as i64;
            if raw < EPOCH {
                0
            } else {
                (raw - EPOCH) / 10_000_000
            }
        };
        let local_unix = unix_from_ymd(lt.wYear as i64, lt.wMonth as i64, lt.wDay as i64)
            + (lt.wHour as i64) * 3600
            + (lt.wMinute as i64) * 60
            + (lt.wSecond as i64) as i64;
        local_unix - utc_unix
    })
}

/// Portable (Linux/macOS) local-timezone offset in seconds east of UTC, resolved
/// once per process. Computed as `mktime(localtime_r(now)) - now` so it works
/// on any libc (glibc/musl/BSD) without tm_gmtoff.
#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static CACHE: OnceLock<i64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            let t = now as libc::time_t;
            libc::localtime_r(&t, &mut tm);
            // mktime interprets the broken-down local time, so the difference
            // from the UTC epoch is the current UTC offset (DST included).
            let local = libc::mktime(&mut tm);
            local - t
        }
    })
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
pub fn tokenize(query: &str, opts: &QueryOptions) -> Vec<Token> {
    // Everything regex mode: the entire search string is one regex.
    if opts.regex && !query.is_empty() {
        return vec![Token::Regex { pattern: query.to_string(), negate: false }];
    }
    let mut tokens: Vec<Token> = Vec::new();
    let mut group: Vec<Token> = Vec::new();
    let mut metric_mode = false;

    let flush_group = |tokens: &mut Vec<Token>, group: &mut Vec<Token>| {
        if group.len() == 1 {
            tokens.push(group.pop().unwrap());
        } else if group.len() > 1 {
            tokens.push(Token::Or(std::mem::take(group)));
        }
        group.clear();
    };

    let parts = split_query(query);
    // Only the `not:` alias needs a scratch buffer; declared uninitialized and
    // filled on first use so the compiler sees no dead initial assignment.
    let mut raw_owned: String;
    for raw in parts {
        if raw == "|" {
            flush_group(&mut tokens, &mut group);
            continue;
        }
        // `metric:` modifier — switches size interpretation to decimal 1000-based.
        if raw.to_ascii_lowercase() == "metric:" {
            metric_mode = true;
            continue;
        }
        // Operator aliases: or: flushes group (same as |),
        // and: strips to plain term, not: is alias for !.
        let mut work = raw.as_str();
        if work.to_ascii_lowercase().starts_with("or:") {
            flush_group(&mut tokens, &mut group);
            work = &work[3..];
        }
        if work.to_ascii_lowercase().starts_with("and:") {
            work = &work[4..];
        }
        if work.to_ascii_lowercase().starts_with("not:") {
            raw_owned = format!("!{}", &work[4..]);
            work = &raw_owned;
        }
        let negate = work.starts_with('!') && !work.starts_with("!<");
        let term = if negate { &work[1..] } else { work };
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let lower = term.to_ascii_lowercase();

        let token = if term.starts_with("regex:") {
            // NFC-normalize (form only, case preserved) so the compiled pattern
            // matches canonical target bytes on macOS; case is the engine flag.
            Token::Regex { pattern: crate::platform::nfc_normalize(term[6..].trim()), negate }
        } else if term.starts_with("case:") {
            // `case:term` — case-sensitive term. NFC-normalize the form so
            // byte-exact comparison against an NFC-normalized name works.
            let pat = crate::platform::nfc_normalize(term[5..].trim());
            if negate {
                Token::Exclude { pattern: pat, whole_word: opts.match_whole_word, case_sensitive: true, anchored_start: false, anchored_end: false }
            } else {
                Token::Include { pattern: pat, whole_word: opts.match_whole_word, case_sensitive: true, anchored_start: false, anchored_end: false }
            }
        } else if term.starts_with("ww:") || term.starts_with("wholeword:") {
            // `ww:term` / `wholeword:term` — whole-word term (Everything
            // modifier; also forced globally by the match_whole_word param).
            let pat = crate::platform::pattern_key(term[term.find(':').unwrap() + 1..].trim(), opts.match_case);
            if negate {
                Token::Exclude { pattern: pat, whole_word: true, case_sensitive: false, anchored_start: false, anchored_end: false }
            } else {
                Token::Include { pattern: pat, whole_word: true, case_sensitive: false, anchored_start: false, anchored_end: false }
            }
        } else if term.starts_with("path:") {
            let p = term[5..].trim().trim_matches('"');
            let norm = crate::platform::canonical_key(&crate::platform::normalize_path(p));
            Token::Path(crate::platform::trim_trailing_sep(&norm).to_string())
        } else if term.starts_with("ext:") {
            let exts: Vec<String> = term[4..]
                .split(',')
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            if !exts.is_empty() {
                Token::Ext(exts)
            } else {
                Token::Never
            }
        } else if lower.starts_with("attrib:") {
            // Everything allows negation both outside (`!attrib:d`) and inside
            // (`attrib:!d`); the two forms combine with XOR.
            let mut rest = &term[7..];
            let mut inner = false;
            if rest.starts_with('!') {
                inner = true;
                rest = &rest[1..];
            }
            match parse_attrib_mask(rest) {
                Some(mask) => Token::Attrib { mask, negate: negate ^ inner },
                None => Token::Never,
            }
        } else if term.starts_with("folder:") {
            Token::TypeFilter(TypeFilter::Folders)
        } else if term.starts_with("file:") {
            Token::TypeFilter(TypeFilter::Files)
        } else if lower.starts_with("is:") {
            // `is:` predicates — Everything shorthand for type/attribute filters.
            match lower[3..].trim() {
                "folder" | "directory" => Token::TypeFilter(TypeFilter::Folders),
                "file" => Token::TypeFilter(TypeFilter::Files),
                "hidden" => Token::Attrib { mask: 0x2, negate },
                "system" => Token::Attrib { mask: 0x4, negate },
                "readonly" | "read_only" | "read-only" => Token::Attrib { mask: 0x1, negate },
                "archive" => Token::Attrib { mask: 0x20, negate },
                "temporary" => Token::Attrib { mask: 0x100, negate },
                "compressed" => Token::Attrib { mask: 0x800, negate },
                "encrypted" => Token::Attrib { mask: 0x4000, negate },
                "offline" => Token::Attrib { mask: 0x1000, negate },
                "reparse" | "reparse_point" => Token::Attrib { mask: 0x400, negate },
                "not_content_indexed" | "not-content-indexed" | "nci" => Token::Attrib { mask: 0x2000, negate },
                "normal" => Token::Attrib { mask: 0x80, negate },
                _ => Token::Never,
            }
        } else if lower.starts_with("size:") {
            match parse_size_filter(&term[5..], metric_mode) {
                Some(f) => Token::Size(f),
                None => Token::Never,
            }
        } else if lower.starts_with("len:") {
            match parse_size_filter(&term[4..], metric_mode) {
                Some(f) => Token::Len(f),
                None => Token::Never,
            }
        } else if lower.starts_with("frn:") {
            match parse_size_filter(&term[4..], false) {
                Some(f) => Token::Frn(f),
                None => Token::Never,
            }
        } else if lower.starts_with("dm:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Modified, op, value: v },
                None => Token::Never,
            }
        } else if lower.starts_with("dc:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Created, op, value: v },
                None => Token::Never,
            }
        } else if lower.starts_with("da:") {
            match parse_date_filter(&term[3..]) {
                Some((op, v)) => Token::Date { kind: DateKind::Accessed, op, value: v },
                None => Token::Never,
            }
        } else if term.starts_with("!<") && term.ends_with('>') {
            Token::Exclude { pattern: crate::platform::pattern_key(&term[2..term.len() - 1], opts.match_case), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: false }
        } else if crate::platform::is_absolute_path(term) {
            let norm = crate::platform::canonical_key(&crate::platform::normalize_path(term));
            Token::BarePath(crate::platform::trim_trailing_sep(&norm).to_string())
        } else if lower.starts_with("start-with:") || lower.starts_with("prefix:") {
            let pat = if lower.starts_with("start-with:") { &term[10..] } else { &term[7..] };
            Token::Include { pattern: crate::platform::pattern_key(pat.trim(), opts.match_case), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: false }
        } else if lower.starts_with("end-with:") || lower.starts_with("suffix:") {
            let pat = if lower.starts_with("end-with:") { &term[9..] } else { &term[7..] };
            Token::Include { pattern: crate::platform::pattern_key(pat.trim(), opts.match_case), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: true }
        } else if negate {
            let mut pat = term;
            let mut anchored_start = false;
            let mut anchored_end = false;
            if pat.starts_with('^') { anchored_start = true; pat = &pat[1..]; }
            if pat.ends_with('$') && !pat.is_empty() { anchored_end = true; pat = &pat[..pat.len()-1]; }
            Token::Exclude { pattern: crate::platform::pattern_key(pat.trim(), opts.match_case), whole_word: opts.match_whole_word, case_sensitive: false, anchored_start, anchored_end }
        } else {
            let mut pat = term;
            let mut anchored_start = false;
            let mut anchored_end = false;
            if pat.starts_with('^') { anchored_start = true; pat = &pat[1..]; }
            if pat.ends_with('$') && !pat.is_empty() { anchored_end = true; pat = &pat[..pat.len()-1]; }
            Token::Include { pattern: crate::platform::pattern_key(pat.trim(), opts.match_case), whole_word: opts.match_whole_word, case_sensitive: false, anchored_start, anchored_end }
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

/// Map Everything `attrib:` letters to FILE_ATTRIBUTE bits. Multiple letters
/// are AND-ed (`attrib:hs` = hidden + system). `n` (normal) is a sentinel
/// (0x80) handled specially by the matcher: matches when no other attribute
/// flags are set. Unknown letters reject the token (`None`).
fn parse_attrib_mask(s: &str) -> Option<u32> {
    const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0080;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0100;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0800;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x2000;
    const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;

    let mut mask = 0u32;
    for c in s.trim().trim_matches('"').chars() {
        let bit = match c.to_ascii_lowercase() {
            'a' => FILE_ATTRIBUTE_ARCHIVE,
            'r' => FILE_ATTRIBUTE_READONLY,
            'h' => FILE_ATTRIBUTE_HIDDEN,
            's' => FILE_ATTRIBUTE_SYSTEM,
            'd' => FILE_ATTRIBUTE_DIRECTORY,
            'n' => FILE_ATTRIBUTE_NORMAL,
            't' => FILE_ATTRIBUTE_TEMPORARY,
            'p' => FILE_ATTRIBUTE_REPARSE_POINT,
            'c' => FILE_ATTRIBUTE_COMPRESSED,
            'o' => FILE_ATTRIBUTE_OFFLINE,
            'i' => FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
            'e' => FILE_ATTRIBUTE_ENCRYPTED,
            _ => return None,
        };
        mask |= bit;
    }
    if mask == 0 {
        return None;
    }
    Some(mask)
}

fn parse_size_filter(s: &str, metric: bool) -> Option<SizeFilter> {
    let s = s.trim();
    // Everything size constants (JEDEC units by default; metric: switches to decimal).
    match s.to_ascii_lowercase().as_str() {
        "tiny" => return Some(SizeFilter::Less(if metric { 1000 } else { 1024 })),
        "small" => return Some(SizeFilter::Less(if metric { 1_000_000 } else { 1024 * 1024 })),
        "medium" => return Some(SizeFilter::Less(if metric { 1_000_000_000 } else { 1024 * 1024 * 1024 })),
        "large" => return Some(SizeFilter::Greater(if metric { 1_000_000_000 } else { 1024 * 1024 * 1024 })),
        "huge" => return Some(SizeFilter::Greater(if metric { 4_000_000_000 } else { 4 * 1024 * 1024 * 1024 })),
        "gigantic" => return Some(SizeFilter::Greater(if metric { 16_000_000_000 } else { 16 * 1024 * 1024 * 1024 })),
        "empty" => return Some(SizeFilter::Equal(0)),
        _ => {}
    }
    if let Some(range) = s.split_once("..") {
        return Some(SizeFilter::Range {
            min: parse_size(range.0)?,
            max: parse_size(range.1)?,
        });
    }
    // Inclusive operators must be checked before their strict counterparts:
    // stripping `>` from `>=1mb` leaves `=1mb`, which would parse as Equal.
    if let Some(v) = s.strip_prefix(">=") {
        return Some(SizeFilter::GreaterOrEqual(parse_size(v)?));
    }
    if let Some(v) = s.strip_prefix("<=") {
        return Some(SizeFilter::LessOrEqual(parse_size(v)?));
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
    // Bare size with a unit suffix is a granularity range to the next unit
    // (`size:1kb` = 1024..2048 per Everything). A bare unitless number is an
    // exact match (`size:100` = exactly 100 bytes).
    if let Some((v, mult)) = parse_size_parts(s, metric) {
        if mult > 1 {
            return Some(SizeFilter::Range { min: v, max: v + mult - 1 });
        }
        return Some(SizeFilter::Equal(v));
    }
    None
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
    if let Some((start, end)) = parse_relative_span(s) {
        return Some((DateOp::Span { start, end }, start));
    }
    Some((DateOp::On, parse_date(s)?))
}

/// Everything-style relative date span: `last7days` / `past7days` /
/// `7days` / `thisweek` / `lastweek` / `thismonth` / `lastyear` etc.
/// Returns `(start_unix, end_unix)` for a `DateOp::Span` window, or `None`
/// if `s` is not a relative span.
fn parse_relative_span(s: &str) -> Option<(i64, i64)> {
    parse_relative_span_at(s, chrono_now())
}

/// Everything-style relative date span evaluated at local time `now`.
///
/// Semantics locked by live probes against Everything:
/// - `lastNdays` / `pastNdays` / bare `Ndays` : rolling window ending now.
/// - `prevNdays` / `previousNdays` : trailing window ending at today's start
///   (`[today_start - N*day, today_start)`), NOT a rolling window.
/// - `lastweek` / `pastweek` : rolling 7 days (Everything treats it as
///   `last7days`); `prevweek` / `previousweek` : calendar previous week
///   (Sunday-start).
/// - `lastmonth` / `pastmonth` : rolling 31 days (probe: equals
///   `last31days` EXACTLY); `prevmonth` / `previousmonth` : calendar month.
/// - `lastyear` / `pastyear` : rolling 365 days; `prevyear` /
///   `previousyear` : calendar previous year.
fn parse_relative_span_at(s: &str, now: i64) -> Option<(i64, i64)> {
    let s = s.trim().to_ascii_lowercase();
    let today = start_of_day(now);
    let span_for = |unit: &str, n: i64| -> i64 {
        match unit {
            "days" => n * 86400,
            "weeks" => n * 7 * 86400,
            "months" => n * 31 * 86400, // Everything: month = 31 days
            "years" => n * 365 * 86400,
            "hours" => n * 3600,
            "minutes" | "mins" => n * 60,
            "seconds" | "secs" => n,
            _ => n * 86400,
        }
    };
    let rolling = |span: i64| Some((now - span, now + 86400));
    let trailing = |span: i64| Some((today - span, today));

    // lastNdays / pastNdays / prevNdays / previousNdays (numeric body).
    for (prefix, roll) in [("last", true), ("past", true), ("prev", false), ("previous", false)] {
        if let Some(body) = s.strip_prefix(prefix) {
            for unit in ["days", "weeks", "months", "years"] {
                if let Some(num) = body.strip_suffix(unit) {
                    if let Ok(n) = num.trim().parse::<i64>() {
                        if n > 0 {
                            let span = span_for(unit, n);
                            return if roll { rolling(span) } else { trailing(span) };
                        }
                    }
                    return None;
                }
            }
            // No numeric body: fall through to the named spans below
            // (`lastweek`, `prevweek`, `lastmonth`, `lastyear`, ...).
        }
    }
    // lastNhours / lastNminutes / lastNseconds (rolling sub-day windows).
    // Everything: `last2hours` = [now - 2h, now]; `prev2hours` = trailing
    // window ending at today's start. Sub-day units use a clean rolling
    // window (no +86400 buffer, which would corrupt a 2-hour span).
    for (prefix, roll) in [("last", true), ("past", true), ("prev", false), ("previous", false)] {
        if let Some(body) = s.strip_prefix(prefix) {
            for unit in ["hours", "minutes", "mins", "seconds", "secs"] {
                if let Some(num) = body.strip_suffix(unit) {
                    if let Ok(n) = num.trim().parse::<i64>() {
                        if n > 0 {
                            let span = span_for(unit, n);
                            return if roll {
                                Some((now - span, now))
                            } else {
                                Some((today - span, today))
                            };
                        }
                    }
                    return None;
                }
            }
        }
    }
    // bare Ndays / Nweeks (Everything accepts `dm:7days` = last 7 days)
    for unit in ["days", "weeks", "months", "years"] {
        if let Some(num) = s.strip_suffix(unit) {
            if let Ok(n) = num.trim().parse::<i64>() {
                if n > 0 {
                    return rolling(span_for(unit, n));
                }
            }
            return None;
        }
    }
    // thisweek / lastweek / prevweek / thismonth / lastmonth / ...
    let week_start = today - (((today / 86400) + 4) % 7) * 86400;
    let (y, m, _) = unix_to_ymd(now);
    let month_start = unix_from_ymd(y, m, 1);
    let prev_month_start = if m == 1 {
        unix_from_ymd(y - 1, 12, 1)
    } else {
        unix_from_ymd(y, m - 1, 1)
    };
    let year_start = unix_from_ymd(y, 1, 1);
    match s.as_str() {
        "thisweek" => Some((week_start, now + 86400)),
        "lastweek" | "pastweek" => rolling(7 * 86400),
        "prevweek" | "previousweek" => Some((week_start - 7 * 86400, week_start)),
        "thismonth" => Some((month_start, now + 86400)),
        "lastmonth" | "pastmonth" => rolling(31 * 86400),
        "prevmonth" | "previousmonth" => Some((prev_month_start, month_start)),
        "thisyear" => Some((year_start, now + 86400)),
        "lastyear" | "pastyear" => rolling(365 * 86400),
        "prevyear" | "previousyear" => Some((unix_from_ymd(y - 1, 1, 1), year_start)),
        // mtd / ytd / qtd: start of current period to now.
        "mtd" => Some((month_start, now)),
        "ytd" => Some((year_start, now)),
        "qtd" => {
            let q_start_month = ((m - 1) / 3) * 3 + 1;
            Some((unix_from_ymd(y, q_start_month, 1), now))
        }
        // Month names: current year's month only (dm:january = January of
        // this year, NOT any January).
        "january" | "jan" => Some((unix_from_ymd(y, 1, 1), unix_from_ymd(y, 2, 1))),
        "february" | "feb" => Some((unix_from_ymd(y, 2, 1), unix_from_ymd(y, 3, 1))),
        "march" | "mar" => Some((unix_from_ymd(y, 3, 1), unix_from_ymd(y, 4, 1))),
        "april" | "apr" => Some((unix_from_ymd(y, 4, 1), unix_from_ymd(y, 5, 1))),
        "may" => Some((unix_from_ymd(y, 5, 1), unix_from_ymd(y, 6, 1))),
        "june" | "jun" => Some((unix_from_ymd(y, 6, 1), unix_from_ymd(y, 7, 1))),
        "july" | "jul" => Some((unix_from_ymd(y, 7, 1), unix_from_ymd(y, 8, 1))),
        "august" | "aug" => Some((unix_from_ymd(y, 8, 1), unix_from_ymd(y, 9, 1))),
        "september" | "sep" => Some((unix_from_ymd(y, 9, 1), unix_from_ymd(y, 10, 1))),
        "october" | "oct" => Some((unix_from_ymd(y, 10, 1), unix_from_ymd(y, 11, 1))),
        "november" | "nov" => Some((unix_from_ymd(y, 11, 1), unix_from_ymd(y, 12, 1))),
        "december" | "dec" => Some((unix_from_ymd(y, 12, 1), unix_from_ymd(y + 1, 1, 1))),
        // Day names: current week's day (Sunday-start), inferred from the
        // month-name pattern (docs are silent; Everything's week is Sunday
        // start, confirmed by the prevweek probe).
        "sunday" | "sun" => Some((week_start, week_start + 86400)),
        "monday" | "mon" => Some((week_start + 86400, week_start + 2 * 86400)),
        "tuesday" | "tue" => Some((week_start + 2 * 86400, week_start + 3 * 86400)),
        "wednesday" | "wed" => Some((week_start + 3 * 86400, week_start + 4 * 86400)),
        "thursday" | "thu" => Some((week_start + 4 * 86400, week_start + 5 * 86400)),
        "friday" | "fri" => Some((week_start + 5 * 86400, week_start + 6 * 86400)),
        "saturday" | "sat" => Some((week_start + 6 * 86400, week_start + 7 * 86400)),
        _ => None,
    }
}

fn unix_to_ymd(unix: i64) -> (i64, i64, i64) {
    let mut days = unix.div_euclid(86400);
    let mut year = 1970i64;
    loop {
        let yd = if is_leap(year) { 366 } else { 365 };
        if days < yd {
            break;
        }
        days -= yd;
        year += 1;
    }
    let mut month = 1i64;
    loop {
        let md = days_in_month(year, month);
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn unix_from_ymd(year: i64, month: i64, day: i64) -> i64 {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month(year, m);
    }
    (days + day - 1) * 86400
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

/// Everything-style wildcard match.
///
/// Supported wildcards (matching Everything 1.5):
///   `*`  any number of chars, does not cross `\`
///   `**` any number of chars, including `\`
///   `?`  exactly one char (not `\`)
///   `[set]` / `[!set]` one char in / not in the set, with `a-z` ranges
///   `#`  exactly one ASCII digit
///   `\x` escapes the next char (only meaningful before a wildcard char)
///
/// Case-insensitive by default. Patterns without wildcard chars fall back to
/// a plain substring search (no `\` escape handling, preserving literal
/// path-like patterns such as `C:\foo`).
#[inline]
pub fn wildcard_match(pattern: &str, text: &str, match_case: bool) -> bool {
    if !pattern.contains(['*', '?', '[', '#']) {
        return if match_case {
            text.contains(pattern)
        } else {
            contains_ci(text, pattern)
        };
    }
    let wcs = parse_wildcard_pattern(pattern);
    if match_case {
        wc_rec(&wcs, text.as_bytes(), 0, 0, &mut HashMap::new())
    } else {
        wc_rec_ci(&wcs, text.as_bytes(), 0, 0, &mut HashMap::new())
    }
}

#[derive(Clone, Debug)]
enum Wc {
    Star { cross_path: bool },
    Q,
    Digit,
    OneOf { set: Vec<(u8, u8)>, negate: bool },
    Lit(u8),
}

/// Parse a wildcard pattern into tokens. `\` escapes only when it precedes a
/// wildcard character; otherwise it is kept as a literal `\` so path-like
/// patterns keep working.
fn parse_wildcard_pattern(pattern: &str) -> Vec<Wc> {
    let b = pattern.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'*' => {
                let cross_path = i + 1 < b.len() && b[i + 1] == b'*';
                let mut j = i;
                while j < b.len() && b[j] == b'*' {
                    j += 1;
                }
                out.push(Wc::Star { cross_path });
                i = j;
            }
            b'?' => {
                out.push(Wc::Q);
                i += 1;
            }
            b'#' => {
                out.push(Wc::Digit);
                i += 1;
            }
            b'[' => match parse_char_class(b, i) {
                Some((set, negate, next)) => {
                    out.push(Wc::OneOf { set, negate });
                    i = next;
                }
                None => {
                    // Unclosed `[` is a literal.
                    out.push(Wc::Lit(b'['));
                    i += 1;
                }
            },
            b'\\' if i + 1 < b.len() && matches!(b[i + 1], b'*' | b'?' | b'[' | b'#' | b'\\') => {
                out.push(Wc::Lit(b[i + 1]));
                i += 2;
            }
            _ => {
                out.push(Wc::Lit(c));
                i += 1;
            }
        }
    }
    out
}

/// Parse `[set]` / `[!set]` starting at `i` (which points at `[`). Returns the
/// set as a range list, the negate flag, and the index just past the closing
/// `]`. `]` as the first member (after optional `!`) is treated as a literal.
/// Parse `[set]` / `[!set]` starting at `i` (which points at `[`). Returns the
/// set as a range list, the negate flag, and the index just past the closing
/// `]`. `]` as the first member (after optional `!`) is treated as a literal.
fn parse_char_class(b: &[u8], i: usize) -> Option<(Vec<(u8, u8)>, bool, usize)> {
    let mut j = i + 1;
    let negate = j < b.len() && b[j] == b'!';
    if negate {
        j += 1;
    }
    let mut ranges: Vec<(u8, u8)> = Vec::new();
    let mut first = true;
    while j < b.len() {
        if b[j] == b']' && !first {
            j += 1;
            return Some((ranges, negate, j));
        }
        first = false;
        if j + 2 < b.len() && b[j + 1] == b'-' && b[j + 2] != b']' {
            let (lo, hi) = (b[j], b[j + 2]);
            ranges.push((lo.min(hi), lo.max(hi)));
            j += 3;
        } else {
            ranges.push((b[j], b[j]));
            j += 1;
        }
    }
    None
}

fn class_hit(ranges: &[(u8, u8)], c: u8) -> bool {
    ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi)
}

fn class_hit_ci(ranges: &[(u8, u8)], c: u8) -> bool {
    class_hit(ranges, c) || (c.is_ascii_alphabetic() && class_hit(ranges, c.to_ascii_lowercase()))
        || (c.is_ascii_alphabetic() && class_hit(ranges, c.to_ascii_uppercase()))
}

/// Recursive wildcard matcher with memoization on (wc-index, text-index).
/// `Star { cross_path: true }` (i.e. `**`) may consume `\`; single `*` may
/// not. `?` matches any single char except `\`.
fn wc_rec(
    wcs: &[Wc],
    t: &[u8],
    wi: usize,
    ti: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> bool {
    if let Some(&r) = memo.get(&(wi, ti)) {
        return r;
    }
    let r = if wi == wcs.len() {
        ti == t.len()
    } else {
        match &wcs[wi] {
            Wc::Star { cross_path } => {
                // Consume zero or more chars; single `*` stops at `\`.
                let mut ok = wc_rec(wcs, t, wi + 1, ti, memo);
                let mut k = ti;
                while !ok && k < t.len() {
                    if t[k] == b'\\' && !*cross_path {
                        break;
                    }
                    k += 1;
                    ok = wc_rec(wcs, t, wi + 1, k, memo);
                }
                ok
            }
            Wc::Q => ti < t.len() && t[ti] != b'\\' && wc_rec(wcs, t, wi + 1, ti + 1, memo),
            Wc::Digit => ti < t.len() && t[ti].is_ascii_digit() && wc_rec(wcs, t, wi + 1, ti + 1, memo),
            Wc::OneOf { set, negate } => {
                let hit = ti < t.len() && class_hit(set, t[ti]);
                if hit != *negate && ti < t.len() {
                    wc_rec(wcs, t, wi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
            Wc::Lit(c) => ti < t.len() && t[ti] == *c && wc_rec(wcs, t, wi + 1, ti + 1, memo),
        }
    };
    memo.insert((wi, ti), r);
    r
}

fn wc_rec_ci(
    wcs: &[Wc],
    t: &[u8],
    wi: usize,
    ti: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> bool {
    if let Some(&r) = memo.get(&(wi, ti)) {
        return r;
    }
    let r = if wi == wcs.len() {
        ti == t.len()
    } else {
        match &wcs[wi] {
            Wc::Star { cross_path } => {
                let mut ok = wc_rec_ci(wcs, t, wi + 1, ti, memo);
                let mut k = ti;
                while !ok && k < t.len() {
                    if t[k] == b'\\' && !*cross_path {
                        break;
                    }
                    k += 1;
                    ok = wc_rec_ci(wcs, t, wi + 1, k, memo);
                }
                ok
            }
            Wc::Q => ti < t.len() && t[ti] != b'\\' && wc_rec_ci(wcs, t, wi + 1, ti + 1, memo),
            Wc::Digit => ti < t.len() && t[ti].is_ascii_digit() && wc_rec_ci(wcs, t, wi + 1, ti + 1, memo),
            Wc::OneOf { set, negate } => {
                let hit = ti < t.len() && class_hit_ci(set, t[ti]);
                if hit != *negate && ti < t.len() {
                    wc_rec_ci(wcs, t, wi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
            Wc::Lit(c) => {
                ti < t.len() && eq_ci(*c, t[ti]) && wc_rec_ci(wcs, t, wi + 1, ti + 1, memo)
            }
        }
    };
    memo.insert((wi, ti), r);
    r
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

fn token_matches(
    entry: &IndexedFile,
    token: &Token,
    compiled: &HashMap<String, (regex::Regex, bool)>,
    opts: &QueryOptions,
) -> bool {
    let name = entry.name.as_str();
    let lower_path = entry.lower_path.as_str();
    let lower_name = entry.lower_name.as_str();

    // Matching target: canonical bytes (NFC + Unicode-lower on macOS) for
    // case-insensitive matching, NFC-normalized original case for
    // case-sensitive matching. `lower_path`/`lower_name` are pre-canonicalized
    // at index build; the cs variant is built on demand (rare, opt-in).
    let tgt = |cs: bool| -> std::borrow::Cow<'_, str> {
        if cs {
            std::borrow::Cow::Owned(crate::platform::nfc_normalize(if opts.match_path {
                &entry.path
            } else {
                name
            }))
        } else if opts.match_path {
            std::borrow::Cow::Borrowed(lower_path)
        } else {
            std::borrow::Cow::Borrowed(lower_name)
        }
    };

    match token {
        Token::Include { pattern, whole_word, case_sensitive, anchored_start, anchored_end } => {
            let cs = opts.match_case || *case_sensitive;
            let target = tgt(cs);
            if *anchored_start && *anchored_end {
                // Exact match.
                if cs { target == pattern.as_str() } else { target.eq_ignore_ascii_case(pattern) }
            } else if *anchored_start {
                if cs { target.starts_with(pattern.as_str()) } else { target.to_ascii_lowercase().starts_with(&pattern.to_ascii_lowercase()) }
            } else if *anchored_end {
                if cs { target.ends_with(pattern.as_str()) } else { target.to_ascii_lowercase().ends_with(&pattern.to_ascii_lowercase()) }
            } else if *whole_word {
                whole_word_match(&target, pattern, cs)
            } else {
                wildcard_match(pattern, &target, cs)
            }
        }
        Token::Exclude { pattern, whole_word, case_sensitive, anchored_start, anchored_end } => {
            // Match against the whole path for `!<dir>` and against the
            // name for plain `!term`.
            let cs = opts.match_case || *case_sensitive;
            let hit = if pattern.contains('\\') || pattern.contains('/') {
                if cs {
                    crate::platform::nfc_normalize(&entry.path).contains(pattern.as_str())
                } else {
                    contains_ci(&entry.lower_path, pattern)
                }
            } else if *anchored_start && *anchored_end {
                let target = tgt(cs);
                if cs { target == pattern.as_str() } else { target.eq_ignore_ascii_case(pattern) }
            } else if *anchored_start {
                let target = tgt(cs);
                if cs { target.starts_with(pattern.as_str()) } else { target.to_ascii_lowercase().starts_with(&pattern.to_ascii_lowercase()) }
            } else if *anchored_end {
                let target = tgt(cs);
                if cs { target.ends_with(pattern.as_str()) } else { target.to_ascii_lowercase().ends_with(&pattern.to_ascii_lowercase()) }
            } else if *whole_word {
                let target = tgt(cs);
                whole_word_match(&target, pattern, cs)
            } else {
                let target = tgt(cs);
                wildcard_match(pattern, &target, cs)
            };
            !hit
        }
        Token::Regex { pattern, negate } => {
            // Regex targets the canonical path when scoped; otherwise the
            // display name. The pattern itself is NFC-normalized at tokenize.
            let hit = compiled
                .get(pattern)
                .map(|(re, _)| re.is_match(if opts.match_path { lower_path } else { name }))
                .unwrap_or(false);
            if *negate {
                !hit
            } else {
                hit
            }
        }
        Token::Or(group) => group.iter().all(|t| token_matches(entry, t, compiled, opts)),
        Token::Path(p) | Token::BarePath(p) => {
            // `p` is already canonical + native-separator at tokenize; the
            // stored lower_path is canonical too, so byte-wise compare works.
            // Boundary must accept both separators (macOS uses `/`).
            let mut ok = lower_path.starts_with(p);
            if ok && lower_path.len() > p.len() {
                ok = matches!(lower_path.as_bytes()[p.len()], b'\\' | b'/' | b':');
            }
            ok
        }
        Token::Ext(exts) => {
            let ext = entry.extension.as_deref().unwrap_or_default();
            exts.iter().any(|e| e == &ext)
        }
        Token::Size(f) => size_match(f, entry.size),
        Token::Len(f) => size_match(f, entry.name.len() as u64),
        Token::Frn(f) => size_match(f, entry.file_ref),
        Token::Date { kind, op, value } => {
            let ts = match kind {
                DateKind::Modified => entry.modified,
                DateKind::Created => entry.created,
                DateKind::Accessed => entry.accessed,
            };
            let unix = windows_ts_to_unix(ts) + local_offset_secs();
            date_match(op, *value, unix)
        }
        Token::TypeFilter(tf) => match tf {
            TypeFilter::Files => !entry.is_dir,
            TypeFilter::Folders => entry.is_dir,
        },
        Token::Attrib { mask, negate } => {
            // `n` (NORMAL, 0x80) is a sentinel: matches only when no other
            // attribute flags are set (Everything's `attrib:n` semantics).
            let hit = if *mask == 0x80 {
                entry.attributes == 0 || entry.attributes == 0x80
            } else {
                entry.attributes & mask == *mask
            };
            if *negate { !hit } else { hit }
        }
        Token::Never => false,
    }
}

fn file_matches(
    entry: &IndexedFile,
    tokens: &[Token],
    compiled: &HashMap<String, (regex::Regex, bool)>,
    opts: &QueryOptions,
) -> bool {
    // Semantics: tokens inside one group (no `|`) are AND-ed; groups joined
    // by `|` are OR-ed. `Token::Or` holds a single AND-group.
    if tokens.is_empty() {
        return true;
    }
    tokens.iter().any(|t| token_matches(entry, t, compiled, opts))
}

fn size_match(f: &SizeFilter, size: u64) -> bool {
    match f {
        SizeFilter::Greater(v) => size > *v,
        SizeFilter::GreaterOrEqual(v) => size >= *v,
        SizeFilter::Less(v) => size < *v,
        SizeFilter::LessOrEqual(v) => size <= *v,
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
        DateOp::Span { start, end } => ts >= *start && ts < *end,
    }
}

/// NTFS 100ns intervals since 1601 -> unix seconds.
/// Exclude token matching, mirroring Everything's semantics:
/// - a token containing a path separator matches as a full path prefix
///   (e.g. `C:\Windows\WinSxS` excludes that subtree)
/// - a bare token matches any path component of that name
///   (e.g. `node_modules` or `target` excludes every folder of that name)
/// `lp` and `p` must both be canonical (NFC + lower on macOS, ASCII-lower
/// elsewhere). Separator detection accepts both `\\` and `/` so the same
/// helpers serve macOS paths.
fn path_excluded(lp: &[u8], p: &[u8]) -> bool {
    if p.contains(&b'\\') || p.contains(&b'/') {
        starts_with_ci_bytes(lp, p)
            && (lp.len() == p.len() || matches!(lp.get(p.len()), Some(b'\\') | Some(b'/')))
    } else {
        dir_name_match(lp, p)
    }
}

fn dir_name_match(lp: &[u8], p: &[u8]) -> bool {
    let mut i = 0;
    while i + p.len() <= lp.len() {
        // Segment start: path start, after a separator, or after the drive
        // letter colon (drive-root children like `C:\target`).
        let seg_start = i == 0 || matches!(lp[i - 1], b'\\' | b'/' | b':');
        if seg_start && lp[i..i + p.len()] == *p {
            let after = i + p.len();
            if after == lp.len() || matches!(lp[after], b'\\' | b'/') {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn starts_with_ci_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() >= b.len() && a[..b.len()].eq_ignore_ascii_case(b)
}

fn windows_ts_to_unix(ts: i64) -> i64 {
    const EPOCH: i64 = 116_444_736_000_000_000;
    if ts < EPOCH {
        return 0;
    }
    ((ts - EPOCH) / 10_000_000) as i64
}

/// Run a search against a snapshot.
/// Shared match filter for search and aggregate: applies the default-exclude
/// gate, path scope, exclude_path, and the query tokens. Returns references to
/// every matching entry in the index (allocating a Vec of pointers only).
fn filter_matches<'a>(
    entries: &'a HashMap<String, IndexedFile>,
    opts: &QueryOptions,
) -> Vec<&'a IndexedFile> {
    let tokens = tokenize(&opts.query, opts);
    let scope_raw = opts.path.as_deref().unwrap_or("");
    let scope = crate::platform::normalize_path(scope_raw);
    let scope_lower = crate::platform::trim_trailing_sep(&crate::platform::canonical_key(&scope)).to_string();
    let exclude_parts: Vec<String> = opts
        .exclude_path
        .as_deref()
        .unwrap_or("")
        .split(';')
        .map(|p| {
            let norm = crate::platform::normalize_path(p.trim());
            crate::platform::trim_trailing_sep(&crate::platform::canonical_key(&norm)).to_string()
        })
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
            if starts_with_ci(lp, &scope_lower) {
                let ok = lp.len() == scope_lower.len()
                    || matches!(lp.as_bytes().get(scope_lower.len()), Some(b'\\') | Some(b'/') | Some(b':'));
                if !ok {
                    continue;
                }
            } else {
                continue;
            }
        }
        if !exclude_parts.is_empty() {
            let lp = entry.lower_path.as_bytes();
            let mut excluded = false;
            for p in &exclude_parts {
                if path_excluded(lp, p.as_bytes()) {
                    excluded = true;
                    break;
                }
            }
            if excluded {
                continue;
            }
        }
        if let Some(content_paths) = &opts.content_paths {
            if !content_paths.iter().any(|cp| cp.as_bytes() == entry.lower_path.as_bytes()) {
                continue;
            }
        }
        if !opts.query.is_empty() && !file_matches(entry, &tokens, &compiled, opts) {
            continue;
        }
        matched.push(entry);
    }
    matched
}

pub fn search(entries: &HashMap<String, IndexedFile>, opts: &QueryOptions) -> QueryResult {
    let mut matched = filter_matches(entries, opts);
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

/// Aggregation query: runs the same filter as [`search`] but returns summary
/// statistics (counts, size sums, per-extension breakdown, largest entries)
/// instead of a paged result list.
pub fn aggregate(
    entries: &HashMap<String, IndexedFile>,
    opts: &AggregateOptions,
) -> AggregateResult {
    let qopts = QueryOptions {
        query: opts.query.clone(),
        path: opts.path.clone(),
        exclude_path: opts.exclude_path.clone(),
        include_all: opts.include_all,
        match_path: opts.match_path,
        regex: opts.regex,
        match_case: opts.match_case,
        match_whole_word: opts.match_whole_word,
        ..Default::default()
    };
    let matched = filter_matches(entries, &qopts);

    let mut files = 0usize;
    let mut folders = 0usize;
    let mut total_size = 0u64;
    let mut by_ext: HashMap<String, AggregateExt> = HashMap::new();
    let mut largest: Vec<AggregateLargest> = Vec::new();

    for e in &matched {
        if e.is_dir {
            folders += 1;
        } else {
            files += 1;
        }
        total_size = total_size.saturating_add(e.size);
        if !e.is_dir {
            let ext = e
                .extension
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase();
            let slot = by_ext.entry(ext.clone()).or_insert_with(|| AggregateExt {
                extension: ext,
                count: 0,
                size: 0,
            });
            slot.count += 1;
            slot.size = slot.size.saturating_add(e.size);
        }
        largest.push(AggregateLargest {
            path: e.path.clone(),
            size: e.size,
            is_dir: e.is_dir,
        });
    }

    largest.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    let top = if opts.top == 0 { 20 } else { opts.top };
    largest.truncate(top);

    let mut by_extension: Vec<AggregateExt> = by_ext.into_values().collect();
    by_extension.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.extension.cmp(&b.extension))
    });

    AggregateResult {
        total: matched.len(),
        files,
        folders,
        total_size,
        largest,
        by_extension,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ft(unix: i64) -> i64 {
        (unix + 116_444_736_00i64) * 10_000_000
    }

    /// Local noon 2026-08-01, offset-independent (safe inside the local day).
    fn aug1_noon() -> i64 {
        parse_date("2026-08-01").unwrap() + 12 * 3600 - local_offset_secs()
    }

    fn entry(path: &str, is_dir: bool, modified: i64) -> IndexedFile {
        let m = ft(modified);
        let mut e = IndexedFile::new(path.to_string(), 0, m, m, m, is_dir, 0);
        e.modified = m;
        e
    }

    fn run(query: &str) -> Vec<String> {
        let map: HashMap<String, IndexedFile> = HashMap::from([
            (".gitignore".to_string(), entry(r"B:\p\.gitignore", false, 1_700_000_000)),
            (".env".to_string(), entry(r"B:\p\.env", false, 1_700_000_000)),
            ("AGENTS.md".to_string(), entry(r"B:\p\AGENTS.md", false, aug1_noon())),
            ("demo.gif".to_string(), entry(r"B:\p\demo.gif", false, aug1_noon())),
            ("readme.md".to_string(), entry(r"B:\p\readme.md", false, aug1_noon())),
            ("docs".to_string(), entry(r"B:\p\docs", true, aug1_noon())),
            ("target".to_string(), entry(r"B:\p\target\debug\x.o", false, 1_700_000_000)),
            ("roottarget".to_string(), entry(r"C:\target\root.o", false, 1_700_000_000)),
            ("rootrecycle".to_string(), entry(r"C:\$Recycle.Bin\gone.txt", false, 1_700_000_000)),
            ("projectsfoo".to_string(), entry(r"B:\pfoo\sibling.txt", false, 1_700_000_000)),
            ("CaseFile.txt".to_string(), entry(r"B:\p\CaseFile.txt", false, 1_700_000_000)),
        ]);
        let opts = QueryOptions { query: query.to_string(), ..Default::default() };
        let r = search(&map, &opts);
        let mut paths: Vec<String> = r.entries.iter().map(|e| e.name.clone()).collect();
        paths.sort();
        paths
    }

    #[test]
    fn and_semantics_within_group() {
        // Two tokens in one group must AND, not OR (regression: the group was
        // wrapped in Token::Or and evaluated with ANY, so `file: dm:today`
        // returned the union of files OR today's files).
        assert_eq!(run("AGENTS.md demo.gif"), Vec::<String>::new());
        assert_eq!(run("AGENTS.md | demo.gif"), vec!["AGENTS.md", "demo.gif"]);
    }

    #[test]
    fn type_filter_and_date_and() {
        // `file: dm:2026-08-01` — files modified on 2026-08-01 (unix 1785542400).
        let got = run("file: dm:2026-08-01");
        assert_eq!(got, vec!["AGENTS.md", "demo.gif", "readme.md"]);
        // dm:2026-08-01 alone must still include dirs.
        assert!(run("dm:2026-08-01").contains(&"docs".to_string()));
        // Bare term is a substring match, dirs match too.
        assert_eq!(run("file: md"), vec!["AGENTS.md", "readme.md"]);
    }

    #[test]
    fn attrib_filter() {
        const HIDDEN: u32 = 0x0002;
        const SYSTEM: u32 = 0x0004;
        const DIRECTORY: u32 = 0x0010;
        let mut dotenv = entry(r"B:\p\.env", false, 1_700_000_000);
        dotenv.attributes = HIDDEN;
        let mut sysfile = entry(r"B:\p\pagefile.sys", false, 1_700_000_000);
        sysfile.attributes = SYSTEM;
        let mut docs = entry(r"B:\p\docs", true, aug1_noon());
        docs.attributes = DIRECTORY;
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("dotenv".to_string(), dotenv),
            ("sysfile".to_string(), sysfile),
            ("docs".to_string(), docs),
        ]);
        let opts = |q: &str| QueryOptions { query: q.to_string(), ..Default::default() };
        let got = |q: &str| {
            let r = search(&map, &opts(q));
            let mut v: Vec<String> = r.entries.iter().map(|e| e.name.clone()).collect();
            v.sort();
            v
        };
        // attrib:h matches only the hidden file; combinable with file:.
        assert_eq!(got("attrib:h"), vec![".env"]);
        assert_eq!(got("file: attrib:s"), vec!["pagefile.sys"]);
        // attrib:d matches the directory; attrib:!d excludes it.
        assert_eq!(got("attrib:d"), vec!["docs"]);
        assert_eq!(got("attrib:!d"), vec![".env", "pagefile.sys"]);
        // Unknown letters reject the token entirely (falls through to bare match).
        assert_eq!(got("attrib:q"), Vec::<String>::new());
    }

    #[test]
    fn invalid_filter_tokens_match_nothing() {
        // One entry so the assertions are meaningful: an empty index would
        // pass trivially.
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("readme.md".to_string(), entry(r"B:\p\readme.md", false, 1_700_000_000)),
        ]);
        let opts = |q: &str| QueryOptions { query: q.to_string(), ..Default::default() };
        let got = |q: &str| {
            let r = search(&map, &opts(q));
            let mut v: Vec<String> = r.entries.iter().map(|e| e.name.clone()).collect();
            v.sort();
            v
        };
        // Invalid filters must match nothing (Token::Never), NOT everything.
        // (Everything returns zero results for an invalid filter; dropping the
        // token would make the query match the whole index.)
        assert_eq!(got("size:garbage"), Vec::<String>::new());
        assert_eq!(got("len:garbage"), Vec::<String>::new());
        assert_eq!(got("frn:garbage"), Vec::<String>::new());
        assert_eq!(got("dm:garbage"), Vec::<String>::new());
        assert_eq!(got("dc:garbage"), Vec::<String>::new());
        assert_eq!(got("da:garbage"), Vec::<String>::new());
        assert_eq!(got("ext:"), Vec::<String>::new());
    }

    #[test]
    fn wildcard_extensions() {
        // `**` crosses `\\`, single `*` does not.
        assert!(wildcard_match("**.rs", r"src\main.rs", false));
        assert!(!wildcard_match("*.rs", r"src\main.rs", false));
        assert!(wildcard_match("*.rs", "main.rs", false));
        // `?` matches one char (not `\\`).
        assert!(wildcard_match("a?c", "abc", false));
        assert!(!wildcard_match("a?c", "ac", false));
        // `[set]` and `[!set]` with ranges.
        assert!(wildcard_match("file[0-9].txt", "file5.txt", false));
        assert!(!wildcard_match("file[0-9].txt", "filex.txt", false));
        assert!(wildcard_match("file[!0-9].txt", "filex.txt", false));
        assert!(!wildcard_match("file[!0-9].txt", "file5.txt", false));
        // `#` matches a single digit.
        assert!(wildcard_match("img#.png", "img7.png", false));
        assert!(!wildcard_match("img#.png", "imgx.png", false));
        // `\\` escapes a wildcard char.
        assert!(wildcard_match(r"a\*b", "a*b", false));
        assert!(!wildcard_match(r"a\*b", "axb", false));
        // Case-insensitive by default; case-sensitive when requested.
        assert!(wildcard_match("*.TXT", "readme.txt", false));
        assert!(!wildcard_match("*.TXT", "readme.txt", true));
    }

    #[test]
    fn anchor_prefix_suffix() {
        let entry = entry("main.rs", false, 1_700_000_000);
        let compiled = HashMap::new();
        let opts = QueryOptions::default();

        // ^ anchors to start of filename.
        let tok = Token::Include { pattern: "main".into(), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: false };
        assert!(token_matches(&entry, &tok, &compiled, &opts));
        let tok = Token::Include { pattern: "main.rs".into(), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: false };
        assert!(token_matches(&entry, &tok, &compiled, &opts));
        // Should not match if pattern is not at start.
        let tok = Token::Include { pattern: ".rs".into(), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: false };
        assert!(!token_matches(&entry, &tok, &compiled, &opts));

        // $ anchors to end of filename.
        let tok = Token::Include { pattern: ".rs".into(), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: true };
        assert!(token_matches(&entry, &tok, &compiled, &opts));
        let tok = Token::Include { pattern: "main.rs".into(), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: true };
        assert!(token_matches(&entry, &tok, &compiled, &opts));
        // Should not match if pattern is not at end.
        let tok = Token::Include { pattern: "main.".into(), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: true };
        assert!(!token_matches(&entry, &tok, &compiled, &opts));

        // ^$ combined = exact match.
        let tok = Token::Include { pattern: "main.rs".into(), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: true };
        assert!(token_matches(&entry, &tok, &compiled, &opts));
        let tok = Token::Include { pattern: "Main.rs".into(), whole_word: false, case_sensitive: true, anchored_start: true, anchored_end: true };
        assert!(!token_matches(&entry, &tok, &compiled, &opts));

        // Exclude anchors work too.
        let tok = Token::Exclude { pattern: ".rs".into(), whole_word: false, case_sensitive: false, anchored_start: false, anchored_end: true };
        assert!(!token_matches(&entry, &tok, &compiled, &opts)); // ends with .rs -> excluded
        let tok = Token::Exclude { pattern: "main".into(), whole_word: false, case_sensitive: false, anchored_start: true, anchored_end: false };
        assert!(!token_matches(&entry, &tok, &compiled, &opts)); // starts with main -> excluded
    }

    #[test]
    fn operator_aliases() {
        // and: is a no-op prefix (AND is default).
        let results = run("and:readme");
        assert!(results.contains(&"readme.md".to_string()));

        // not: is alias for !
        let results = run("not:readme.md");
        assert!(!results.contains(&"readme.md".to_string()));

        // or: flushes group (same as |).
        let results = run("readme or:demo");
        assert!(results.contains(&"readme.md".to_string()));
        assert!(results.contains(&"demo.gif".to_string()));
    }

    #[test]
    fn exclude_path_bare_token_drive_root() {
        // Regression: a bare exclude token must match a directory directly
        // under the drive root (`C:\target\...`), where the preceding byte
        // is the drive colon, not a backslash.
        let opts = QueryOptions {
            query: "*".to_string(),
            exclude_path: Some("target".to_string()),
            include_all: true,
            ..Default::default()
        };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("roottarget".to_string(), entry(r"C:\target\root.o", false, 1_700_000_000)),
            ("rootrecycle".to_string(), entry(r"C:\$Recycle.Bin\gone.txt", false, 1_700_000_000)),
        ]);
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].path, r"C:\$Recycle.Bin\gone.txt");
    }

    #[test]
    fn default_excludes_drive_root() {
        // Regression: `$Recycle.Bin` at the drive root must be excluded by
        // default (its name sits directly after the drive colon).
        let opts = QueryOptions { query: "*".to_string(), ..Default::default() };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("roottarget".to_string(), entry(r"C:\target\root.o", false, 1_700_000_000)),
            ("rootrecycle".to_string(), entry(r"C:\$Recycle.Bin\gone.txt", false, 1_700_000_000)),
        ]);
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].path, r"C:\target\root.o");
    }

    #[test]
    fn path_scope_forward_slashes() {
        // A foreign separator in a scope must behave like the native one.
        // Windows indexes use drive-rooted backslash paths; Unix indexes use
        // slash-rooted paths. Keep the fixture native so this tests the
        // production path representation on every target.
        let inside_path = if cfg!(windows) { r"C:\Users\a.txt" } else { "/Users/a.txt" };
        let other_path = if cfg!(windows) { r"C:\Windows\a.txt" } else { "/Windows/a.txt" };
        let scopes: &[&str] = if cfg!(windows) {
            &[r"C:\Users", "C:/Users"]
        } else {
            &["/Users", r"\Users"]
        };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("inside".to_string(), entry(inside_path, false, 1_700_000_000)),
            ("other".to_string(), entry(other_path, false, 1_700_000_000)),
        ]);
        for scope in scopes {
            let opts = QueryOptions {
                query: "*".to_string(),
                path: Some((*scope).to_string()),
                ..Default::default()
            };
            let r = search(&map, &opts);
            let paths: Vec<&str> = r.entries.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(paths, vec![inside_path], "scope {scope:?}");
        }
    }

    #[test]
    fn path_scope_is_directory_boundary() {
        // Regression: scope `B:\p` (or `/p`) must not match its sibling
        // `B:\pfoo\...` (or `/pfoo/...`).
        let scope = if cfg!(windows) { r"B:\p" } else { "/p" };
        let inside_path = if cfg!(windows) { r"B:\p\a.txt" } else { "/p/a.txt" };
        let sibling_path = if cfg!(windows) { r"B:\pfoo\b.txt" } else { "/pfoo/b.txt" };
        let opts = QueryOptions {
            query: "*".to_string(),
            path: Some(scope.to_string()),
            ..Default::default()
        };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("inside".to_string(), entry(inside_path, false, 1_700_000_000)),
            ("sibling".to_string(), entry(sibling_path, false, 1_700_000_000)),
        ]);
        let r = search(&map, &opts);
        let paths: Vec<&str> = r.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec![inside_path]);
    }

    #[test]
    fn case_prefix_is_case_sensitive() {
        // Regression: `case:` must force case-sensitive matching for that
        // token even when the global match_case flag is off.
        assert_eq!(run("case:CaseFile.txt"), vec!["CaseFile.txt"]);
        assert_eq!(run("case:casefile.txt"), Vec::<String>::new());
        assert_eq!(run("case:Case*"), vec!["CaseFile.txt"]);
        // Non-case: token remains case-insensitive.
        assert_eq!(run("casefile.txt"), vec!["CaseFile.txt"]);
    }

    #[test]
    fn size_units_are_jedec() {
        // Regression: Everything's default size standard is JEDEC (kb=1024,
        // mb=1024^2, gb=1024^3); the native engine used decimal 1000-based
        // units, so `size:1mb` missed files of exactly 1 MiB.
        let opts = QueryOptions { query: "size:1mb".to_string(), ..Default::default() };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("exact".to_string(), entry(r"B:\p\exact.bin", false, 1_700_000_000)),
            ("decimal".to_string(), entry(r"B:\p\decimal.bin", false, 1_700_000_000)),
        ]);
        let mut exact = map["exact"].clone();
        exact.size = 1024 * 1024;
        let mut decimal = map["decimal"].clone();
        decimal.size = 1_000_000;
        let map = HashMap::from([
            ("exact".to_string(), exact),
            ("decimal".to_string(), decimal),
        ]);
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].path, r"B:\p\exact.bin");
        // Plain byte values are unaffected.
        let opts = QueryOptions { query: "size:1000000".to_string(), ..Default::default() };
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].path, r"B:\p\decimal.bin");
    }

    #[test]
    fn size_inclusive_operators() {
        // Regression: `size:>=1mb` and `size:<=1mb` were parsed as `=1mb`
        // (the `>`/`<` was stripped first), so `>=` fell back to Equal and
        // `<=` to Less. Both inclusive forms must now work.
        let mut one_mb = entry(r"B:\p\one.bin", false, 1_700_000_000);
        one_mb.size = 1024 * 1024;
        let mut two_mb = entry(r"B:\p\two.bin", false, 1_700_000_000);
        two_mb.size = 2 * 1024 * 1024;
        let map = HashMap::from([
            ("one".to_string(), one_mb),
            ("two".to_string(), two_mb),
        ]);
        let r = search(&map, &QueryOptions {
            query: "size:>=1mb".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 2, ">=1mb must include both");
        let r = search(&map, &QueryOptions {
            query: "size:>1mb".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, ">1mb must exclude exactly-1mb");
        let r = search(&map, &QueryOptions {
            query: "size:<=1mb".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, "<=1mb must include only 1mb");
    }

    #[test]
    fn size_bare_unit_is_granularity_range() {
        // Regression: Everything treats a bare unit value as a range up to
        // the next unit (`size:1kb` = 1024..2047), not an exact match. The
        // unitless `size:100` remains an exact match.
        let mut at = entry(r"B:\p\at.bin", false, 1_700_000_000);
        at.size = 1024; // 1kb, the low edge
        let mut over = entry(r"B:\p\over.bin", false, 1_700_000_000);
        over.size = 2048; // 2kb, just past the 1kb range
        let map = HashMap::from([
            ("at".to_string(), at),
            ("over".to_string(), over),
        ]);
        let r = search(&map, &QueryOptions {
            query: "size:1kb".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, "size:1kb must be 1024..2047");
        assert_eq!(r.entries[0].path, r"B:\p\at.bin");
        let r = search(&map, &QueryOptions {
            query: "size:1024".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, "size:1024 exact");
        let r = search(&map, &QueryOptions {
            query: "size:1024..2048".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 2, "explicit range 1024..2048");
    }

    #[test]
    fn size_constants() {
        // Everything size constants: tiny < 1KB, small < 1MB, medium < 1GB,
        // large > 1GB, huge > 4GB, gigantic > 16GB, empty == 0.
        let mk = |name: &str, size: u64| {
            let mut e = entry(&format!(r"B:\p\{name}"), false, 1_700_000_000);
            e.size = size;
            (name.to_string(), e)
        };
        let map = HashMap::from([
            mk("zero", 0),
            mk("halfk", 512),
            mk("onek", 1024),
            mk("onem", 1024 * 1024),
            mk("oneg", 1024 * 1024 * 1024),
            mk("twog", 2 * 1024 * 1024 * 1024),
            mk("fiveg", 5 * 1024 * 1024 * 1024),
            mk("twentyg", 20 * 1024 * 1024 * 1024),
        ]);
        let got = |q: &str| {
            let r = search(&map, &QueryOptions { query: q.to_string(), ..Default::default() });
            let mut v: Vec<String> = r.entries.iter().map(|e| e.name.clone()).collect();
            v.sort();
            v
        };
        assert_eq!(got("size:tiny"), vec!["halfk", "zero"]);
        assert_eq!(got("size:small"), vec!["halfk", "onek", "zero"]);
        assert_eq!(got("size:medium"), vec!["halfk", "onek", "onem", "zero"]);
        assert_eq!(got("size:large"), vec!["fiveg", "twentyg", "twog"]);
        assert_eq!(got("size:huge"), vec!["fiveg", "twentyg"]);
        assert_eq!(got("size:gigantic"), vec!["twentyg"]);
        assert_eq!(got("size:empty"), vec!["zero"]);
    }

    #[test]
    fn date_rolling_vs_calendar() {
        // Regression: `dm:lastweek` is a ROLLING 7-day window (Everything
        // treats it as `last7days`), NOT the calendar previous week; only
        // `dm:prevweek` is the calendar week. Also `dm:lastmonth` is rolling
        // 31 days, `dm:prevmonth` is the calendar month.
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let entry_mod = |path: &str, ts: i64| {
            let mut e = entry(path, false, ts);
            e.modified = ft(ts);
            e
        };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            // modified now (in the rolling week)
            ("now".to_string(), entry_mod(r"B:\p\now.txt", now - 3600)),
            // 10 days ago: in rolling lastweek, not in prevweek
            ("10d".to_string(), entry_mod(r"B:\p\ten.txt", now - 10 * 86400)),
            // 40 days ago: in rolling lastmonth, not in prevmonth
            ("40d".to_string(), entry_mod(r"B:\p\forty.txt", now - 40 * 86400)),
        ]);
        let r = search(&map, &QueryOptions {
            query: "dm:lastweek".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, "lastweek = rolling 7d");
        assert_eq!(r.entries[0].path, r"B:\p\now.txt");
        let r = search(&map, &QueryOptions {
            query: "dm:pastweek".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 1, "pastweek = rolling 7d");
        let r = search(&map, &QueryOptions {
            query: "dm:lastmonth".to_string(),
            ..Default::default()
        });
        assert_eq!(r.entries.len(), 2, "lastmonth = rolling 31d (now + 10d)");
        let r = search(&map, &QueryOptions {
            query: "dm:prevmonth".to_string(),
            ..Default::default()
        });
        // 10 days ago falls in the calendar previous month (July, from Aug);
        // 40 days ago and now do not.
        assert_eq!(r.entries.len(), 1, "prevmonth = calendar previous month");
        assert_eq!(r.entries[0].path, r"B:\p\ten.txt");
    }

    #[test]
    fn date_prev_nd_is_trailing_not_rolling() {
        // Regression: `dm:prev7days` is a trailing window ending at today's
        // start (everything between today_start-7d and today_start), NOT a
        // rolling window ending at now. `last7days`/`7days` stay rolling.
        // Use a fixed `now` so the assertion is time-of-day independent.
        let now = 1_785_600_000 + 5 * 3600; // 2026-08-04T05:00:00Z (local)
        let today = start_of_day(now);
        let cases = [
            ("last7days", now - 2 * 3600, true, "rolling: earlier today"),
            ("prev7days", now - 2 * 3600, false, "trailing: earlier today >= today_start"),
            ("last7days", today - 2 * 86400, true, "rolling: 2d before today"),
            ("prev7days", today - 2 * 86400, true, "trailing: 2d before today"),
            ("last7days", now - 30 * 86400, false, "rolling: 30d ago"),
            ("prev7days", now - 30 * 86400, false, "trailing: 30d ago"),
        ];
        for (token, ts, expect, why) in cases {
            let (start, end) = parse_relative_span_at(token, now).unwrap();
            let got = ts >= start && ts < end;
            assert_eq!(got, expect, "{token} @ {ts}: {why} (window {start}..{end})");
        }
    }

    #[test]
    fn date_constants() {
        // Everything date constants: month names (current year only), day
        // names (current week, Sunday-start), mtd/ytd/qtd, and rolling
        // lastNhours/lastNminutes/lastNseconds. Fixed `now` for determinism.
        let now = 1_785_600_000 + 5 * 3600; // 2026-08-04T05:00:00Z (local)
        let today = start_of_day(now);
        let week_start = today - (((today / 86400) + 4) % 7) * 86400;
        let (y, m, _) = unix_to_ymd(now);
        let month_start = unix_from_ymd(y, m, 1);
        let year_start = unix_from_ymd(y, 1, 1);
        let q_start = unix_from_ymd(y, ((m - 1) / 3) * 3 + 1, 1);

        // Month names: current year's month only.
        let (s, e) = parse_relative_span_at("january", now).unwrap();
        assert_eq!((s, e), (unix_from_ymd(y, 1, 1), unix_from_ymd(y, 2, 1)));
        let (s, e) = parse_relative_span_at("aug", now).unwrap();
        assert_eq!((s, e), (unix_from_ymd(y, 8, 1), unix_from_ymd(y, 9, 1)));
        let (s, e) = parse_relative_span_at("dec", now).unwrap();
        assert_eq!((s, e), (unix_from_ymd(y, 12, 1), unix_from_ymd(y + 1, 1, 1)));

        // Day names: current week's day (Sunday-start).
        let (s, e) = parse_relative_span_at("sunday", now).unwrap();
        assert_eq!((s, e), (week_start, week_start + 86400));
        let (s, e) = parse_relative_span_at("sat", now).unwrap();
        assert_eq!((s, e), (week_start + 6 * 86400, week_start + 7 * 86400));

        // mtd / ytd / qtd: start of current period to now.
        let (s, e) = parse_relative_span_at("mtd", now).unwrap();
        assert_eq!((s, e), (month_start, now));
        let (s, e) = parse_relative_span_at("ytd", now).unwrap();
        assert_eq!((s, e), (year_start, now));
        let (s, e) = parse_relative_span_at("qtd", now).unwrap();
        assert_eq!((s, e), (q_start, now));

        // Rolling sub-day windows.
        let (s, e) = parse_relative_span_at("last2hours", now).unwrap();
        assert_eq!((s, e), (now - 2 * 3600, now));
        let (s, e) = parse_relative_span_at("last30minutes", now).unwrap();
        assert_eq!((s, e), (now - 30 * 60, now));
        let (s, e) = parse_relative_span_at("last45secs", now).unwrap();
        assert_eq!((s, e), (now - 45, now));
    }

    #[test]
    fn relative_date_spans() {
        // Regression: `dm:last7days` / `dm:7days` are ranges (last 7 days),
        // not a single day 7 days ago; `dm:thisweek` spans the current week.
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let entry_mod = |path: &str, ts: i64| {
            let mut e = entry(path, false, ts);
            e.modified = ft(ts);
            e
        };
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("today".to_string(), entry_mod(r"B:\p\today.txt", now)),
            ("2days".to_string(), entry_mod(r"B:\p\two.txt", now - 2 * 86400)),
            ("10days".to_string(), entry_mod(r"B:\p\ten.txt", now - 10 * 86400)),
        ]);
        let opts = QueryOptions { query: "dm:last7days".to_string(), ..Default::default() };
        let r = search(&map, &opts);
        let paths: Vec<&str> = r.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec![r"B:\p\today.txt", r"B:\p\two.txt"]);
        let opts = QueryOptions { query: "dm:7days".to_string(), ..Default::default() };
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 2);
        let opts = QueryOptions { query: "dm:yesterday".to_string(), ..Default::default() };
        let r = search(&map, &opts);
        assert_eq!(r.entries.len(), 0);
    }

    #[test]
    fn absolute_date_uses_local_midnight() {
        // Regression: `dm:2026-08-01` must match local-midnight boundaries,
        // not UTC. `aug1` is the wall-clock-unix of local midnight Aug 1; a
        // file one minute before it belongs to dm:2026-07-31, one minute after
        // belongs to dm:2026-08-01 — regardless of the host timezone.
        let aug1 = parse_date("2026-08-01").unwrap();
        // entry() stores `modified` as UTC unix; wall-clock = modified + offset.
        // A file whose wall-clock is local midnight Aug 1 has modified =
        // aug1 - local_offset_secs(). One minute either side of that boundary
        // belongs to dm:2026-07-31 / dm:2026-08-01 regardless of timezone.
        let before = aug1 - local_offset_secs() - 60;
        let after = aug1 - local_offset_secs() + 60;
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("before".to_string(), entry(r"B:\p\before.txt", false, before)),
            ("after".to_string(), entry(r"B:\p\after.txt", false, after)),
        ]);
        let opts = QueryOptions { query: "dm:2026-08-01".to_string(), ..Default::default() };
        let r = search(&map, &opts);
        let paths: Vec<&str> = r.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec![r"B:\p\after.txt"]);
    }

    #[test]
    fn aggregate_largest_ext_and_size() {
        let aug1_noon = parse_date("2026-08-01").unwrap() + 12 * 3600 - local_offset_secs();
        let mut e1 = entry(r"B:\p\big.zip", false, aug1_noon);
        e1.size = 5_000;
        let mut e2 = entry(r"B:\p\med.zip", false, aug1_noon);
        e2.size = 3_000;
        let mut e3 = entry(r"B:\p\small.txt", false, aug1_noon);
        e3.size = 1_000;
        let mut e4 = entry(r"B:\p\tiny.txt", false, aug1_noon);
        e4.size = 500;
        let d = entry(r"B:\p\folder", true, aug1_noon);
        let map: HashMap<String, IndexedFile> = HashMap::from([
            ("big".to_string(), e1),
            ("med".to_string(), e2),
            ("small".to_string(), e3),
            ("tiny".to_string(), e4),
            ("folder".to_string(), d),
        ]);
        let opts = AggregateOptions {
            query: "file:".to_string(),
            top: 3,
            ..Default::default()
        };
        let a = aggregate(&map, &opts);
        assert_eq!(a.total, 4);
        assert_eq!(a.files, 4);
        assert_eq!(a.folders, 0);
        assert_eq!(a.total_size, 9_500);
        let largest: Vec<(u64, &str)> =
            a.largest.iter().map(|l| (l.size, l.path.as_str())).collect();
        assert_eq!(
            largest,
            vec![
                (5_000, r"B:\p\big.zip"),
                (3_000, r"B:\p\med.zip"),
                (1_000, r"B:\p\small.txt"),
            ]
        );
        let mut by_ext: Vec<(&str, usize, u64)> = a
            .by_extension
            .iter()
            .map(|e| (e.extension.as_str(), e.count, e.size))
            .collect();
        by_ext.sort_by_key(|(ext, _, _)| ext.to_string());
        assert_eq!(by_ext, vec![("txt", 2, 1_500), ("zip", 2, 8_000)]);
    }
}

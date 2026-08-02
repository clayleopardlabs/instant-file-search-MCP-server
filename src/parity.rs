//! Differential parity harness: runs identical queries through the native
//! indexer pipe and the Everything engine, comparing counts.
//!
//! Requires the instant-file-search-indexer service RUNNING and the
//! Everything engine reachable. Run manually:
//!   cargo test --release -p instant-file-search-mcp-server -- --ignored parity
//!
//! Mismatches are reported (not failed) so the battery always completes.

#[cfg(test)]
mod parity_tests {
    use crate::everything;
    use crate::native;
    use crate::tools::CountParams;

    #[test]
    #[ignore]
    fn parity_battery() {
        // (query, path_scope) pairs. The path is passed via the `path` param
        // (a folder scope in both engines), NOT as a `path:` query token:
        // in Everything `path:` is a full-path match modifier, in native it
        // is a Token::Path — they are not the same operation.
        let queries: &[(&str, Option<&str>)] = &[
            ("AGENTS.md", None),
            ("*.png", None),
            ("*.json", None),
            ("cargo.toml", None),
            ("size:>100mb", None),
            ("size:0", None),
            ("size:1mb", None),
            ("size:1kb..10kb", None),
            ("dm:today", None),
            ("dm:yesterday", None),
            ("dm:last7days", None),
            ("dm:7days", None),
            ("dm:thisweek", None),
            ("dm:lastweek", None),
            ("dm:thismonth", None),
            ("dc:today", None),
            ("ext:pdf", None),
            ("ext:png ext:jpg", None),
            ("file:", None),
            ("folder:", None),
            ("case:README", None),
            ("readme", None),
            ("*.rs | *.py", None),
            ("A B C", None),
            ("size:>1mb", None),
            ("size:>=1mb", None),
            ("size:<=1mb", None),
            ("size:1mb..1mb", None),
            ("size:1mb..2mb", None),
            ("dm:lastmonth", None),
            ("dm:lastyear", None),
            ("dm:prevweek", None),
            ("dm:pastweek", None),
            ("file:", Some(r"C:\")),
            ("file:", Some(r"B:\")),
            ("file:", Some(r"D:\")),
            ("file:", Some(r"C:\Windows")),
            ("file:", Some(r"C:\Users")),
            ("file:", Some(r"C:\Program Files")),
            ("file:", Some(r"C:\ProgramData")),
            ("file:", Some(r"C:\Windows\WinSxS")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\node_modules")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\.git")),
            ("file:", Some(r"C:\Users\Omen\AppData\Roaming\node_modules")),
            ("node_modules", Some(r"C:\Users")),
            (".gitignore", None),
            (".git", Some(r"C:\")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local")),
            ("file:", Some(r"C:\Users\Omen\AppData\Roaming")),
            ("file:", Some(r"C:\Users\Omen\Desktop")),
            ("file:", Some(r"C:\Users\Omen\Downloads")),
            ("file:", Some(r"C:\Users\Omen\.config")),
            ("file:", Some(r"C:\Users\Omen\.lmstudio")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Temp")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Packages")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Microsoft")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Programs")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\D3DSCache")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\CrashDumps")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\hermes")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Autodesk")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Python")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\npm-cache")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\wsl")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\Google")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\AnkiProgramFiles")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\AnkiProgramFiles\.venv")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\AnkiProgramFiles\python")),
            ("file:", Some(r"C:\Users\Omen\AppData\Local\AnkiProgramFiles\cache")),
            ("size:>1mb", Some(r"C:\")),
            ("size:>1mb", Some(r"B:\")),
            ("size:>100mb", Some(r"C:\")),
            ("size:>100mb", Some(r"B:\")),
            ("size:1mb", Some(r"C:\")),
            ("size:>100mb", Some(r"C:\Users")),
            ("size:>100mb", Some(r"C:\Windows")),
            ("size:>100mb", Some(r"C:\Program Files")),
            ("size:>100mb", Some(r"C:\Program Files (x86)")),
            ("size:>100mb", Some(r"C:\ProgramData")),
            ("size:>100mb", Some(r"C:\Users\Omen")),
            ("size:>100mb", Some(r"C:\Users\Omen\AppData")),
            ("size:>100mb", Some(r"C:\Users\Omen\Videos")),
            ("size:>100mb", Some(r"C:\Users\Omen\Downloads")),
            ("size:>100mb", Some(r"C:\Users\Omen\VirtualBox VMs")),
            ("size:>100mb", Some(r"C:\Users\Omen\.ollama")),
            ("size:>100mb", Some(r"C:\Users\Omen\.lmstudio")),
            ("size:>100mb", Some(r"B:\Games")),
            ("size:>100mb", Some(r"B:\Vortex Mods")),
            ("size:>100mb", Some(r"B:\Projects")),
            ("size:>100mb", Some(r"B:\Videos")),
            ("size:>100mb", Some(r"B:\ISO")),
            ("size:>1mb", Some(r"C:\Users")),
            ("size:>1mb", Some(r"C:\Windows")),
            ("size:>1mb", Some(r"C:\Program Files")),
            ("size:>1mb", Some(r"B:\Games")),
            ("size:>1mb", Some(r"B:\Projects")),
            ("size:>1mb", Some(r"B:\Vortex Mods")),
            ("file:", Some(r"B:\Games")),
            ("file:", Some(r"B:\Projects")),
            ("file:", Some(r"B:\Vortex Mods")),
            ("file:", Some(r"B:\Documents")),
            ("file:", Some(r"B:\Software")),
            ("file:", Some(r"B:\Mods")),
            ("file:", Some(r"B:\opencode")),
            ("file:", Some(r"B:\oh-my-openagent")),
            ("file:", Some(r"B:\Shared Folder")),
            ("file:", Some(r"B:\Projects\instant-file-search-MCP-server")),
            ("file:", Some(r"B:\Projects\leopardlabswebsite")),
            ("file:", Some(r"B:\Projects\lims-equipment-protocols")),
            ("file:", Some(r"B:\Projects\HPLCledLamp")),
            ("file:", Some(r"B:\Projects\BlackHole")),
            ("file:", Some(r"B:\Projects\opencode-reddit-scraper")),
            ("file:", Some(r"B:\Projects\Hunyuan3D-2")),
            ("file:", Some(r"B:\Projects\opencode-reddit-plugin-private")),
            ("file:", Some(r"B:\Projects\the-cavemans-interpreter")),
            ("file:", Some(r"B:\Projects\Cultivar")),
            ("file:", Some(r"B:\Projects\plain-board")),
            ("file:", Some(r"B:\Projects\currentAgentForTesting")),
            ("file:", Some(r"B:\Projects\mullvad-429-rotate")),
            ("file:", Some(r"B:\Projects\ste100-compiler")),
            ("file: dm:lastyear", None),
            ("folder: dm:lastyear", None),
            ("dm:2026-07-01", None),
            ("file: dm:2026-07-01", None),
            ("folder: dm:2026-07-01", None),
            ("dm:2026-07-02", None),
        ];
        let mut mismatches = 0usize;
        let mut native_fails = 0usize;
        let mut everything_fails = 0usize;
        let params = |q: &str, p: Option<&str>| CountParams {
            query: q.to_string(),
            path: p.map(|s| s.to_string()),
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for (q, p) in queries {
            let n = native::count(&params(q, *p));
            let e = everything::count(params(q, *p));
            let (nv, nn) = match n {
                Ok(v) => (v, String::new()),
                Err(err) => {
                    native_fails += 1;
                    ((0, String::new()), err.to_string())
                }
            };
            let (ev, en) = match e {
                Ok(v) => (v, String::new()),
                Err(err) => {
                    everything_fails += 1;
                    ((0, String::new()), err.to_string())
                }
            };
            let (nv, _nn0) = nv;
            let (ev, _en0) = ev;
            let mark = if nv == ev && nn.is_empty() && en.is_empty() {                "OK  "
            } else {
                mismatches += 1;
                "DIFF"
            };
            println!(
                "{mark} {} native={nv:<10} everything={ev:<10}{}{}",
                match p {
                    Some(path) => format!("{q:<16} in {path}"),
                    None => format!("{q:<22}"),
                },
                if nn.is_empty() {
                    String::new()
                } else {
                    format!(" native_err={nn}")
                },
                if en.is_empty() {
                    String::new()
                } else {
                    format!(" everything_err={en}")
                }
            );
        }
        println!(
            "\nmismatches={mismatches} native_fails={native_fails} everything_fails={everything_fails}"
        );
    }

    #[test]
    #[ignore]
    fn parity_dump_everything() {
        for q in [
            "size:>100mb path:C:\\Users\\Omen\\.ollama",
            "size:>100mb path:C:\\Program Files",
            "size:>100mb path:C:\\Program Files (x86)",
            "size:0 path:C:\\Windows",
            "size:0 path:C:\\Users\\Omen\\Desktop",
            "file: B:\\Projects\\instant-file-search-MCP-server",
            "file: B:\\Projects\\instant-file-search-MCP-server\\plugin\\node_modules",
            "file: B:\\Projects\\instant-file-search-MCP-server !<node_modules\\>",
            "file: !<node_modules\\> B:\\Projects\\instant-file-search-MCP-server",
            "dm:2026-07-01",
            "dm:2026-06-01",
            "file: dm:2026-07-01",
            "dm:2026-07-02",
            "file: dm:2026-07-02",
            "page-2026-07-02T00-00-04-868Z dm:2026-07-02",
            "page-2026-07-02T00-00-04-868Z dm:2026-07-01",
            "folder: dm:2026-07-02",
            "folder: dm:2026-07-01",
        ] {
            match crate::everything::search(crate::tools::SearchParams {
                query: q.to_string(),
                path: None,
                regex: None,
                match_case: None,
                match_whole_word: None,
                exclude_path: None,
                include_all: None,
                match_path: None,
                fields: None,
                sort: None,
                max_results: Some(200),
                offset: Some(0),
            }) {
                Ok(res) => {
                    println!("=== Everything {q} total={}", res.total);
                    for e in res.results {
                        let p = e.path.clone().unwrap_or_default();
                        let d = e.date_modified.clone().unwrap_or_default();
                        println!("  {:?}  {}{}  m={}", e.size, p, e.filename, d);
                    }
                }
                Err(err) => println!("=== Everything {q} ERR: {err}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn parity_probe_semantics() {
        let params = |q: &str| CountParams {
            query: q.to_string(),
            path: None,
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for q in [
            "size:100",
            "size:1000",
            "size:1024",
            "size:1kb",
            "size:2kb",
            "size:1mb",
            "size:2mb",
            "size:1mb..2mb",
            "size:1mb..3mb",
            "size:5mb",
            "size:1gb",
            "size:1000000",
            "size:1048576",
            "size:1.5mb",
            "dm:last30days",
            "dm:last90days",
            "dm:last365days",
            "dm:prev7days",
            "dm:prev30days",
            "dm:prev365days",
            "dm:prevweek",
            "dm:pastweek",
            "dm:prevmonth",
            "dm:pastmonth",
            "dm:prevyear",
            "dm:pastyear",
            "dm:last2weeks",
            "dm:last2months",
            "dm:previousweek",
            "dm:previousmonth",
            "dm:previousyear",
        ] {
            match crate::everything::count(params(q)) {
                Ok((v, _)) => println!("E {q:<20} = {v}"),
                Err(err) => println!("E {q:<20} ERR: {err}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn parity_probe_dates() {
        let params = |q: &str| CountParams {
            query: q.to_string(),
            path: None,
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for q in [
            "dm:1/7/2026..31/7/2026",
            "dm:1/8/2026..1/8/2026",
            "dm:lastmonth",
            "dm:prevmonth",
            "dm:thismonth",
            "dm:lastweek",
            "dm:prevweek",
            "dm:thisweek",
            "dm:1/1/2026..31/12/2026",
            "dm:1/1/2025..31/12/2025",
            "dm:lastyear",
            "dm:prevyear",
            "size:>=1mb",
            "size:<=1mb",
            "size:<1mb",
            "size:>1mb",
            "size:=1mb",
            "size:==1mb",
            "size:1mb",
            "size:100..200",
            "size:>100",
            "size:>=100",
            "size:<100",
            "size:<=100",
        ] {
            match crate::everything::count(params(q)) {
                Ok((v, _)) => println!("E {q:<22} = {v}"),
                Err(err) => println!("E {q:<22} ERR: {err}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn parity_probe_calendar() {
        let params = |q: &str| CountParams {
            query: q.to_string(),
            path: None,
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for q in [
            "dm:2026-07-01..2026-07-31",
            "dm:2026-07-01..2026-08-01",
            "dm:2026-08-01..2026-08-31",
            "dm:2025-01-01..2025-12-31",
            "dm:2026-01-01..2026-12-31",
            "dm:prevmonth",
            "dm:prevyear",
            "dm:lastmonth",
            "dm:lastyear",
            "dm:prev7days",
            "dm:last7days",
            "dm:prev14days",
            "dm:last14days",
            "dm:prevmonth 2026-07-01..2026-07-31",
        ] {
            match crate::everything::count(params(q)) {
                Ok((v, _)) => println!("E {q:<42} = {v}"),
                Err(err) => println!("E {q:<42} ERR: {err}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn parity_probe_lastmonth() {
        let params = |q: &str| CountParams {
            query: q.to_string(),
            path: None,
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for q in [
            "dm:lastmonth",
            "dm:last30days",
            "dm:last31days",
            "dm:2026-07-01..2026-07-31",
            "dm:2026-07-02..2026-07-31",
            "dm:2026-07-03..2026-07-31",
            "dm:2026-07-04..2026-07-31",
            "dm:2026-07-05..2026-07-31",
            "dm:2026-07-06..2026-07-31",
            "dm:2026-07-07..2026-07-31",
            "dm:2026-07-01..2026-07-30",
            "dm:2026-07-01..2026-07-29",
            "dm:2026-07-01..2026-07-28",
            "dm:2026-07-02..2026-08-01",
            "dm:2026-07-03..2026-08-01",
            "dm:2026-07-04..2026-08-01",
            "dm:2026-07-05..2026-08-01",
            "dm:2026-07-06..2026-08-01",
            "dm:2026-07-07..2026-08-01",
        ] {
            match crate::everything::count(params(q)) {
                Ok((v, _)) => println!("E {q:<42} = {v}"),
                Err(err) => println!("E {q:<42} ERR: {err}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn parity_probe_weeks() {
        let params = |q: &str| CountParams {
            query: q.to_string(),
            path: None,
            regex: None,
            match_case: None,
            match_whole_word: None,
            exclude_path: None,
            include_all: None,
        };
        for q in [
            "dm:prev7days",
            "dm:prevweek",
            "dm:thisweek",
            "dm:2026-07-18..2026-07-25",
            "dm:2026-07-19..2026-07-25",
            "dm:2026-07-20..2026-07-26",
            "dm:2026-07-18..2026-07-24",
            "dm:2026-07-26..2026-08-01",
            "dm:2026-07-27..2026-08-01",
            "dm:2026-07-25..2026-08-01",
            "dm:2026-07-19..2026-07-26",
            "size:1kb..2kb",
            "size:1024..2047",
            "size:1kb..1mb",
            "size:1.5mb..2mb",
            "size:1.5mb..2.5mb",
            "size:2mb..3mb",
            "size:1gb..2gb",
            "size:1000..1001",
            "size:1kb..1kb",
        ] {
            match crate::everything::count(params(q)) {
                Ok((v, _)) => println!("E {q:<28} = {v}"),
                Err(err) => println!("E {q:<28} ERR: {err}"),
            }
        }
    }
}

# Demo Searches (16 GIFs, pre-verified)

Every prompt below was run live against the engine and confirmed to return
real data, so nothing comes up empty during recording. Read them top to
bottom. The visual impact comes from the numbers (22k, 5k, 148k, 2.5M) and
the structured result cards showing paths, sizes, and dates.

Keep narration to one line per result ("22,181 JSON files", "five biggest
DLLs", etc.): the tool result cards carry the visual weight.

## find_files

1. Find the project's README file.
2. List the source files but ignore dependencies and build output.
3. Find files that look like secrets or local config.
4. Find old installers and archives on this computer.
5. Find the largest files on the machine (over 1 GB).
6. What's been modified in the last 7 days?
7. Find files containing the text `fn main`.
8. Where are all the images in my projects?

## count_files

9. How many JSON files are on this computer?
10. Count how many test files exist beside implementation files.

## count-then-list

11. How many JSON files are there, then show me the first ones in this project.

## recent_changes

12. What changed in this project in the last few hours?
13. Which files were renamed or deleted recently?

## aggregate_files

14. What's in this repo by file type?
15. Which file types take up the most space, and what are the largest files?

## search_status

16. Is the search engine healthy, and how many files are indexed?

---

## Reusable working queries

Use `exclude_path` for noise: the `!target` inline modifier does not exclude
directories in the native engine. Use `|` (pipe) for OR, the `;` separator is
Everything syntax and silently returns zero in the native engine.

1. `find_files query="README.md" path="<project-directory>"`
2. `find_files query="*.rs" path="<project-directory>" exclude_path="target;node_modules;.git"`, 24
3. `find_files query=".env | *.pem | credentials.json | id_ed25519" path="C:/Users/<user>/AppData/Local/Temp/opencode" exclude_path="site-packages"`, 4 seeded demo secrets
4. `find_files query="*.zip | *.iso | *.msi" sort="size" exclude_path="node_modules;WinSxS;.git;target"`, 5,008, biggest 13GB ISOs
5. `find_files query="file: size:>1gb" sort="size" exclude_path="node_modules;WinSxS;.git;AppData"`, 325, 24GB videos / 22GB models
6. `find_files query="dm:7days" sort="date_modified" exclude_path="node_modules;WinSxS;.git"`, 148,622
7. `find_files query="content:\"fn main\"" path="<project-directory>" exclude_path="target"`
8. `find_files query="*.png" path="B:/Projects" exclude_path="target;node_modules;.git"`, 1,444
9. `count_files query="*.json" exclude_path="node_modules;WinSxS;.git"`, 22,181
10. `count_files query="*_test.py | *_test.js | *.test.js" exclude_path="node_modules;.git;target"`, 153
11. `count_files query="*.json" exclude_path="node_modules;WinSxS;.git"` then `find_files query="*.json" path="<project-directory>" exclude_path="target" max_results=10`
12. `recent_changes hours=6`: live WRITE/RENAME/DELETE events
13. `recent_changes hours=24 reasons=renamed,deleted`
14. `aggregate_files query="file:" path="<project-directory>" exclude_path="target;node_modules;.git" top=8`, 76 files, 13.3MB, per-extension breakdown
15. `aggregate_files query="file:" path="B:/Projects" exclude_path="target;node_modules;.git" top=8`, largest files by size
16. `search_status`: 2,513,000 indexed on B:\ + C:\, both engines up

## Notes

- Demo 3 (secrets) is seeded: a small set of realistic credential files lives
  under `C:\Users\<user>\AppData\Local\Temp\opencode\demo-secrets\`
  (`.env` with API keys, `deploy_key_ed25519.pem`, `id_ed25519` SSH key,
  `credentials.json`). The search is scoped to that folder so it always finds
  exactly these four: no dependence on where real secrets live, and no CA
  bundle noise (`site-packages` is excluded).
- Demos 1, 2, 7, 11, 14 are scoped to `<project-directory>` so they work on
  any checkout; the rest search the whole PC for maximum wow.

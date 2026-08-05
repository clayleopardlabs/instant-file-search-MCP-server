# Demo Searches (10 prompts, increasing in impressiveness)

Record a prompt in the left pane, then show the tool results. Read them top to bottom.

1. Find every file named `AGENTS.md` on this PC.

2. List all `.png` files, then count how many there are.

3. Show me the largest 10 files in the project folder (skip build artifacts).

4. Find every `.env` file across all drives, but exclude `node_modules` and `.git`.

5. Which source files in this repo were modified today (skip build artifacts)?

6. Count all the `.json` files on the machine (skip build folders).

7. Find files over 100 MB anywhere on this computer.

8. Search inside `B:\Projects` for Rust source files and count them.

9. Find files whose *path* contains `backup` and were edited this week.

10. Regex search: every file matching `probedemo\d+`, then verify the count dropped to zero after cleanup.

---

## Reusable working queries

Use `exclude_path` for noise — the `!target` inline modifier does not exclude
directories in the native engine.

1. `find_files query="AGENTS.md"`
2. `find_files query="*.png"` followed by `count_files query="*.png"`
3. `find_files query="file:" path="<project-directory>" exclude_path="target" sort="size_desc" max_results=10`
4. `find_files query="*.env" exclude_path="node_modules;.git"`
5. `find_files query="file: dm:today" path="<project-directory>" exclude_path="target"`
6. `count_files query="*.json" exclude_path="target;node_modules"`
7. `find_files query="file: size:>100mb" max_results=50`
8. `count_files query="*.rs" path="<projects-directory>"`
9. `find_files query="backup dm:thisweek" match_path=true`
10. `count_files query="regex:probedemo\d+" regex=true`

For the last query, create a few disposable files matching the pattern in a
temporary directory, count them, remove them, and count again to verify that
the result returns to zero.

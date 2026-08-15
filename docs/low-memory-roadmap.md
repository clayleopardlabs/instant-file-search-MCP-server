# Low-memory index roadmap

Status: implemented through milestone 6; native privileged smoke tests remain
platform-specific follow-up work.

Baseline: `aadf7af` on `master`

This document is the handoff plan for the next implementation session. Read
`AGENTS.md` before starting. Complete the milestones in order. Each milestone
must be tested, committed, and pushed before work begins on the next one.

## Why the order changed

The six proposed improvements are worthwhile, but they should not be
implemented in the original list order.

1. Improve the benchmark harness first so later performance claims have a
   repeatable baseline.
2. Add schema migrations, integrity checks, and recovery before changing the
   SQLite schema for faster queries.
3. Optimize disk queries only after the benchmark and migration foundations
   exist.
4. Add disk-backed content as a separate opt-in feature. Do not enable it
   automatically with disk metadata mode because many low-memory users do not
   want the extra disk space, indexing work, or local copy of file contents.
5. Update installers after all runtime configuration values are stable.
6. Finish with process-restart, crash-window, and privileged platform tests.

Disk metadata mode is the default. Existing installations must keep
their current behavior unless the user explicitly changes a setting.

## Current behavior and baseline

- `INSTANT_FS_INDEX_MODE=disk` is the default.
- `INSTANT_FS_INDEX_MODE=disk` stores file metadata in SQLite.
- Disk metadata mode stores durable Windows USN and macOS FSEvents checkpoints.
- Linux still performs a full metadata walk after service restart because
  fanotify has no persistent event history.
- `INSTANT_FS_CONTENT_INDEX` currently accepts true-like values and controls a
  bounded 256 MiB in-memory content cache.
- Disk metadata mode leaves content indexing off by default. Set
  `INSTANT_FS_CONTENT_INDEX=disk` to use a bounded temporary content cache in the
  same SQLite database.
- The disk query engine streams rows from SQLite and applies the shared Rust
  filter engine. This protects query parity but scans too many rows for common
  filters.
- The current Windows synthetic result for 500,000 records is approximately
  495 MiB RSS in memory mode and 10 MiB RSS in disk mode. Disk queries are
  roughly 2.2 to 6.6 times slower in that workload.
- The current regression suite has 53 passing indexer tests.

Preserve these public MCP tools and their schemas:

- `find_files`
- `count_files`
- `search_status`
- `recent_changes`
- `aggregate_files`

New fields may be added to `search_status`, but existing fields and meanings
must not change.

## Cross-cutting rules

Apply these rules to every milestone:

1. Never create a SQL optimization that can exclude a true match. Complex or
   unsupported queries must fall back to the shared Rust matcher.
2. Keep memory and disk metadata results identical for the same index and
   query, including ordering, totals, pagination, aggregates, default
   exclusions, Unicode paths, and mixed path separators.
3. Do not silently fall back from a requested disk mode to memory mode. That
   could consume RAM the user explicitly tried to preserve.
4. Permission errors, read-only filesystems, disk-full errors, and unsupported
   schema versions are not corruption. Report them and leave the database
   untouched.
5. Automatic database recovery may quarantine a database only after SQLite
   reports `SQLITE_CORRUPT` or `SQLITE_NOTADB`, or an integrity check proves
   corruption.
6. Benchmarks must run in release builds and separate processes when comparing
   RSS. Report warm and reopened database results separately.
7. Synthetic CI benchmarks are regression signals, not universal performance
   claims. Publish user-facing numbers only from named hardware and operating
   systems.
8. Tests that require NTFS raw-volume access, fanotify capabilities, macOS Full
   Disk Access, or service-manager privileges must be explicit live smoke
   tests. Their portable logic must also have unprivileged tests using a fake
   event source.
9. Do not claim literal 100 percent correctness. Report exactly which tools,
   filters, recovery windows, and platforms were exercised.
10. Preserve unrelated work in a dirty worktree.

## Standard validation before every commit

Run the relevant focused tests during development, then run all of these before
each milestone commit:

```text
cargo fmt --check
cargo test --workspace
cargo check --workspace
git diff --check
```

Also run these platform checks when the required runner is available:

- Windows: native `cargo test --workspace` plus PowerShell installer and doctor
  tests.
- Linux: `.github/workflows/linux.yml`, shellcheck, systemd unit verification,
  and a native x86_64 Linux build.
- macOS: `.github/workflows/macos.yml`, shellcheck, `plutil -lint`, and native
  arm64 macOS tests.

After validation:

1. Confirm the intended files are the only staged files.
2. Commit with the milestone message specified below.
3. Push the current branch to `origin`.
4. Confirm the branch is synchronized with its upstream.
5. Only then mark the milestone complete and start the next one.

## Milestone 1: repeatable benchmark and measurement suite

Goal: establish trustworthy measurements before optimization.

Suggested commit:

```text
test: expand index performance benchmarks
```

### Implementation

- Move benchmark code out of `indexer/src/main.rs` into a focused module such
  as `indexer/src/benchmark.rs`.
- Keep the current command syntax working:
  `instant-file-search-indexer benchmark memory 500000`.
- Add a structured syntax that can grow without breaking callers, for example:
  `benchmark synthetic --mode disk --entries 500000 --runs 10 --json`.
- Generate a mixed corpus instead of making every record an `.rs` file. Include:
  - several extensions;
  - files and folders;
  - scoped and excluded paths;
  - multiple sizes, dates, attributes, and volumes;
  - ASCII and Unicode names;
  - both common selective queries and broad all-match queries.
- Measure:
  - initial build time;
  - process RSS after build;
  - close and reopen time;
  - first query after reopen;
  - warm-query p50 and p95 over repeated runs;
  - single-record and batch update latency;
  - metadata database, WAL, and shared-memory file sizes;
  - process read and write bytes when the operating system exposes them.
- Emit one versioned JSON record so results can be compared mechanically.
  Human-readable output can be an additional format.
- Add a read-only live-service benchmark that sends the same request set to the
  installed service. It must not stop, restart, or modify the live service.
- Add an optional watcher-latency smoke test that writes only inside a required
  user-supplied scratch directory. It must require an explicit `--allow-write`
  flag.
- Add small synthetic benchmark smoke runs to CI. Do not enforce absolute time
  thresholds in shared CI. Validate the schema and sane nonzero measurements.
- Add a script or subcommand that compares two JSON result files and reports
  absolute and percentage changes.
- Document the difference between a reopened database and a truly cold OS page
  cache. Do not label a reopened test as a cold-cache test.

### Files likely involved

- `indexer/src/main.rs`
- `indexer/src/benchmark.rs` (new)
- `indexer/src/index.rs`
- platform-specific process metric helpers, if separated
- `.github/workflows/linux.yml`
- `.github/workflows/macos.yml`
- Windows release workflow or a new Windows test workflow
- `docs/development.md`

### Acceptance criteria

- Old benchmark syntax still works.
- Memory and disk runs occur in separate processes for published RSS results.
- JSON output includes a schema version, platform, architecture, build commit,
  storage mode, corpus description, and every measured unit.
- A 10,000-record benchmark smoke test passes on Windows, Linux, and macOS CI.
- Live-service mode is read-only unless the explicit scratch option is given.
- Baseline JSON for the current implementation is saved as a CI artifact or in
  a clearly labeled local results directory that is ignored by Git.

## Milestone 2: SQLite schema, durability, maintenance, and recovery

Goal: make persistent metadata safe to evolve and predictable after crashes.

Suggested commit:

```text
feat: harden disk index durability and recovery
```

### Implementation

- Add explicit schema versions using `PRAGMA user_version`.
- Implement ordered, transactional migrations. Opening a database created by a
  newer unsupported version must fail with a clear message and must not modify
  the file.
- Centralize SQLite connection configuration in `indexer/src/disk.rs`:
  - WAL journal mode;
  - documented synchronous level;
  - finite busy timeout;
  - foreign keys when content tables are introduced;
  - `mmap_size=0` for predictable low process memory;
  - a bounded WAL autocheckpoint policy.
- Keep `synchronous=NORMAL` only if the documented contract states that a power
  failure may replay recent filesystem changes but may not corrupt committed
  structure. Use `FULL` for schema migrations and recovery metadata if needed.
- Run a lightweight startup integrity check such as `quick_check(1)` before
  trusting saved checkpoints.
- On confirmed corruption:
  1. close the connection;
  2. atomically quarantine the main database and existing `-wal` and `-shm`
     sidecars with a timestamped suffix;
  3. create a new empty database;
  4. expose recovery details through logs and `search_status`;
  5. allow the existing missing-checkpoint path to trigger a full scan.
- If quarantine or recreation fails, stop with an actionable error. Never
  delete the only copy.
- Add a maintenance API:
  - passive WAL checkpoint during normal operation;
  - truncate checkpoint during clean shutdown when safe;
  - `PRAGMA optimize` after large scans or migrations;
  - explicit offline `vacuum` or compact command, never surprise automatic
    vacuum during startup.
- Report database schema version, health, WAL bytes, and last recovery reason in
  `search_status`.
- Ensure the database directory is created with service-appropriate restricted
  permissions on each platform. Do not make metadata or future content tables
  world-readable.

### Tests

- Create and reopen every supported schema version fixture.
- Reject a future schema version without modification.
- Verify a migration is atomic when deliberately interrupted.
- Verify permission, read-only, busy, and disk-full-like errors do not trigger
  quarantine.
- Corrupt a disposable test database and verify quarantine plus clean rebuild.
- Verify WAL checkpoint and maintenance commands preserve query parity.
- Verify status fields for healthy, recovered, and failed states.

### Acceptance criteria

- A normal restart does not rebuild a valid database.
- A corrupt disposable database is retained under a quarantine name and the
  service rebuilds a fresh index.
- Non-corruption open failures never rename or delete the database.
- Existing databases from `aadf7af` migrate automatically.
- The benchmark from milestone 1 shows no material memory regression and
  records any build or reopen cost change.

## Milestone 3: SQLite query planning and indexed prefilters

Goal: narrow the disk-mode speed gap while preserving every query result.

Suggested commit:

```text
perf: accelerate disk-backed metadata queries
```

### Design

Use two planner levels:

1. Exact SQL plan for a deliberately small subset whose semantics can be
   reproduced exactly in SQLite.
2. Conservative SQL prefilter for complex queries, followed by the existing
   Rust `EntryFilter` as the final authority.

If the planner is uncertain, select more rows and let Rust reject them. It must
never select fewer rows than the real match set.

### Schema work

Add versioned columns needed for query planning. Likely columns are:

- canonical path key;
- canonical file name key;
- extension;
- default-excluded flag;
- existing numeric metadata already stored in `files`.

Add only indexes justified by query plans and benchmark data. Candidate indexes:

- extension plus file/folder type;
- modified, created, and accessed timestamps;
- size;
- volume plus file reference, which already exists;
- canonical path prefix support.

Measure index file size and build cost after each candidate. Do not add a
metadata FTS table in this milestone. It has a large storage and write cost and
does not naturally preserve the full wildcard grammar.

### Query planner work

- Add a query-planning representation in `indexer/src/query.rs` that does not
  depend directly on rusqlite types.
- Translate safe metadata constraints:
  - explicit path scope;
  - extension;
  - file or folder type;
  - size and file reference ranges;
  - created, modified, and accessed ranges;
  - attribute masks;
  - default-excluded state;
  - simple literal or wildcard name patterns only when escaping and case rules
    are proven equivalent.
- Treat OR groups, regex, duplicate detection, whole-word matching, complex
  character classes, negated expressions, and unsupported wildcard forms as
  Rust-filtered unless an exact translation is proven.
- For exact plans, use SQL for count and aggregate operations and fetch only the
  requested ordered page when SQL ordering is equivalent to the canonical Rust
  order.
- For conservative plans, stream candidate rows through `EntryFilter` and keep
  the current bounded top-N behavior.
- Include a debug-only or benchmark-only explanation of the selected plan and
  indexes. Do not expose internal SQL through the MCP protocol.

### Tests

- Keep the existing memory-versus-disk differential suite.
- Add generated differential tests covering combinations of every filter,
  sorting option, pagination, include-all behavior, mixed separators, Unicode,
  and invalid query tokens.
- Run each generated query with SQL planning enabled and with a forced full-scan
  fallback, then compare complete serialized results.
- Test that unsupported queries visibly choose the fallback plan.
- Test migration backfill of new canonical columns.
- Use `EXPLAIN QUERY PLAN` in focused tests or benchmark diagnostics to confirm
  intended selective queries use the new indexes. Avoid brittle assertions on
  SQLite's full textual plan.

### Performance acceptance criteria

Use milestone 1 JSON results on the same machine and release build.

- No query result, count, aggregate, sort, or page differs from memory mode.
- Memory-mode median performance changes by less than 10 percent.
- Disk-mode RSS remains within 20 MiB for the 500,000-record synthetic corpus.
- The metadata database size and initial build slowdown are reported.
- Common selective extension, path, type, size, and date queries should improve
  by at least 2 times when the corpus contains enough nonmatching rows.
- Broad all-match queries may improve less because their exact totals still
  require examining many records. Report this honestly.
- Update the README benchmark table only after rerunning the complete benchmark
  on named hardware. Keep the README free of em dashes.

## Milestone 4: optional disk content cache

Goal: allow literal content search without a large resident cache while keeping
the feature explicitly opt-in.

Suggested commit:

```text
feat: add optional disk-backed content search
```

### Configuration contract

Extend `INSTANT_FS_CONTENT_INDEX` while preserving old values:

- `1`, `true`, `on`, or `memory`: bounded in-memory content index;
- `0`, `false`, or `off`: content indexing disabled;
- `disk`: temporary disk content cache;
- unset or `auto`: memory content for memory metadata mode and off for disk
  metadata mode, preserving current effective behavior.

Disk content mode should initially require disk metadata mode. Reject the
unsupported combination with a clear error rather than silently allocating a
large path set in memory.

Add explicit limits such as:

- maximum bytes read from one file;
- total cached content bytes;
- background backfill rate or concurrency.

Names may be adjusted during implementation, but defaults and units must be
documented and reported by `search_status`.

### Storage and query design

- Store content in the same protected SQLite database or in a tightly coupled
  database that supports a join without materializing every matching path in
  RAM. Prefer the same database for transactional deletion and rename handling.
- Add a versioned `file_contents` table keyed by file path with source modified
  time, indexed byte count, normalized searchable content, and an eviction or
  recency field.
- Preserve current literal, case-insensitive, all-needles semantics. Do not
  replace them with token-only FTS semantics.
- A later FTS index may be used only as a conservative candidate generator,
  followed by literal verification. It is not required for this milestone.
- Integrate content predicates into the disk metadata query so `find_files`,
  `count_files`, and `aggregate_files` do not build an unbounded in-memory list
  of matching paths.
- Apply file metadata changes and content deletion or rename consistently.
  Prefer one transaction where practical.
- First-time content indexing must run in the background without blocking
  metadata search. Disk content caches start fresh after restart.
- Enforce the disk budget deterministically and expose files indexed, bytes
  stored, eligible files, coverage state, and last backfill error.
- Treat the content database as sensitive. Ensure platform installers create
  restrictive ownership and permissions, and explain that opting in stores a
  searchable local copy of eligible text.

### Tests

- Match current in-memory content results for quotes, spaces, multiple needles,
  case folding, updates, renames, deletions, and ineligible files.
- Verify cache cleanup and query results after reopen.
- Verify backfill interruption.
- Verify budget enforcement and deterministic eviction.
- Verify no unbounded path-vector allocation in disk content mode.
- Verify content remains off by default in disk metadata mode.
- Verify search status accurately reports disabled, memory, building, ready,
  limited, and failed states.

### Acceptance criteria

- Disk metadata plus disk content stays within a measured low-RAM target. Use
  the milestone 1 harness and publish the actual result.
- Metadata-only users pay no content database or backfill cost.
- `content:` works through find, count, and aggregate operations in disk content
  mode with parity against memory content mode.
- Restart clears the disk content cache before rereading eligible files.

## Milestone 5: cross-platform installer and service configuration

Goal: make storage choices durable and discoverable on Windows, Linux, and
macOS.

Suggested commit:

```text
feat: configure index modes in platform installers
```

### Shared behavior

- Expose validated installer choices for metadata mode and content mode.
- Default new installs to metadata `disk` and content `auto`.
- On upgrade, preserve the installed choice when the user does not specify a
  new value.
- Record the selected values in installation state where that platform has
  state, and make diagnostics compare service configuration with live
  `search_status`.
- Restart the service only after configuration files are written atomically.
- Print a concise cost and benefit summary before applying a nondefault mode.

### Windows

- Add parameters to `scripts/install.ps1`, likely `-IndexMode` and
  `-ContentMode`, with strict value validation.
- Store per-service environment variables in the Windows service registry
  `Environment` multi-string value, not in the user's global environment.
- Preserve other service environment entries.
- Record effective values in `current.json`.
- Extend `scripts/doctor.ps1` to verify the registry values, service command,
  active version, and live status agree.
- Test fresh install, idempotent reinstall, upgrade preservation, explicit mode
  change, invalid values, and paths containing spaces or apostrophes.
- Continue to satisfy every installation acceptance criterion in `AGENTS.md`.

### Linux

- Add `--index-mode` and `--content-mode` to `scripts/install-linux.sh`.
- Write a root-owned configuration file such as
  `/etc/default/instant-file-search-indexer` using an atomic replacement.
- Add `EnvironmentFile=-/etc/default/instant-file-search-indexer` to the systemd
  unit.
- Preserve existing values when flags are omitted.
- Run `systemd-analyze verify` when available, then daemon-reload and restart.
- Extend dry-run and shellcheck coverage for every mode combination.

### macOS

- Add `--index-mode` and `--content-mode` to `scripts/install-macos.sh`.
- Render validated values into the installed launchd plist under
  `EnvironmentVariables`, or generate the installed plist from a root-owned
  configuration source. Do not depend on an interactive shell environment.
- Preserve settings across idempotent reinstall and upgrade.
- Validate the final installed plist with `plutil -lint` before bootstrap.
- Keep Full Disk Access instructions accurate for the deployed binary.

### Documentation and diagnostics

- Update README installation examples in simple English. Do not use em dashes
  anywhere added to README.
- Update Windows, Linux, and macOS build pages.
- Document storage locations, permissions, upgrade preservation, how to switch
  modes, expected restart behavior, and how to return to defaults.
- Make `search_status` the source of truth for effective runtime mode.

### Acceptance criteria

- Fresh install, reinstall, upgrade, and explicit mode change work on all three
  operating systems.
- The service receives settings without relying on a user's terminal session.
- Omitting installer flags never resets an existing nondefault mode.
- Invalid settings fail before service configuration changes.
- Windows `doctor.ps1 -RequireNative` continues to pass.
- Linux and macOS CI validate scripts and templates. Final completion also
  requires a real systemd and launchd smoke test on native machines.

## Milestone 6: restart, crash-window, and full parity verification

Goal: prove the persistent design across controlled failure windows and perform
the final cross-platform validation.

Suggested commit:

```text
test: cover persistent index restart and crash recovery
```

### Portable recovery harness

- Extract journal replay decisions behind a small fakeable event-source
  interface. Keep platform watchers responsible for reading native events, but
  test mutation and checkpoint ordering without privileges.
- Add a child-process test helper that operates only on a unique temporary
  directory and database. It should support deliberate exit or abort at named
  crash points.
- Cover these windows:
  1. crash before a metadata transaction commits;
  2. crash after metadata commits but before the checkpoint advances;
  3. crash after checkpoint commit;
  4. crash during schema migration;
  5. crash during content backfill;
  6. stale or replaced Windows USN journal;
  7. unavailable or wrapped macOS FSEvents history;
  8. Linux restart, which must take the documented full-walk path;
  9. truncated WAL and confirmed corrupt main database;
  10. clean shutdown and immediate reopen.
- Verify replay is idempotent. Duplicate replay may repeat work, but must not
  duplicate files, sizes, content, or change state.
- Verify the checkpoint never advances beyond committed metadata and content.

### Tool and filter parity matrix

Run every MCP protocol operation against memory metadata, disk metadata, and
disk metadata plus disk content where applicable:

- ping/status protocol methods;
- find/search;
- count;
- aggregate;
- recent changes;
- content search.

Cover every query token family, option, sort, pagination behavior, invalid
input case, scope, exclusion, and mixed filter combination. Generate a machine
readable matrix showing which cases ran in each mode.

The final report should say all enumerated cases passed. It should not claim
that finite tests prove every possible filesystem state.

### Native live smoke scripts

- Windows NTFS smoke test: use a dedicated scratch directory, create, modify,
  rename, and delete files, restart the service, and validate USN replay.
- macOS APFS smoke test: use a dedicated scratch directory, validate FSEvents
  replay, restart launchd, and separately report Full Disk Access coverage.
- Linux smoke test: use a dedicated scratch directory, validate fanotify live
  updates, restart systemd, and validate the documented full-walk recovery.
- Require explicit confirmation or flags before restarting an installed
  service. Never modify files outside the named scratch directory.
- Record OS version, filesystem, architecture, binary commit, active modes, and
  whether privileged coverage was available.

### Final performance and documentation pass

- Rerun milestone 1 on the same Windows hardware used for the baseline.
- Run equivalent release measurements on at least one native Apple Silicon Mac
  and one representative Linux machine.
- Compare old memory, new memory, disk metadata, and disk metadata plus disk
  content.
- Report build, reopen, update, query p50/p95, RSS, process I/O, and database
  sizes.
- Update the README table with clearly labeled hardware and methodology. Keep
  the README free of em dashes.
- Update architecture and platform documents with final recovery guarantees and
  limitations.

### Acceptance criteria

- Every portable crash-window test passes in CI.
- Every enumerated tool and filter parity case passes in all applicable modes.
- Windows live smoke passes before final completion.
- macOS and Linux code can be merged with native CI, but release claims about
  privileged watchers require the real-machine smoke results to be attached.
- Final benchmark JSON and a readable comparison are retained as release or CI
  artifacts.
- The repository is clean, every milestone commit is on the remote branch, and
  the final branch matches its upstream.

## Final definition of done

All work is complete only when:

1. Six milestone commits have each been tested and pushed independently.
2. Existing MCP schemas remain compatible.
3. Disk mode remains the default and shows no material regression.
4. Disk metadata mode has measured low RAM, faster common queries, durable
   schema handling, and safe corruption recovery.
5. Disk content is opt-in, temporary, bounded, secure, and does not create an
   unbounded in-memory candidate list.
6. Windows, Linux, and macOS installers preserve and verify the selected mode.
7. The full tool/filter matrix and portable crash tests pass.
8. Native privileged smoke coverage and its limitations are reported honestly.
9. Final benchmark and README claims identify the hardware and method used.

## Progress checklist

- [x] Milestone 1 committed and pushed
- [x] Milestone 2 committed and pushed
- [x] Milestone 3 committed and pushed
- [x] Milestone 4 committed and pushed
- [x] Milestone 5 committed and pushed
- [x] Milestone 6 committed and pushed

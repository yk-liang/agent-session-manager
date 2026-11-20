# Repository Conventions

## Data Versioning

Only official releases are compatibility milestones—never dev commits, local installs, or test ledgers. Check release history and commits before changing any version.

- **Persistent formats** (`PRAGMA user_version`, custom pricing JSON, config transaction logs): Assign one target version per release cycle for incompatible changes and reuse it until release. Upgrade each released format directly to the target; never chain intermediate dev formats. Initialize empty databases at the current schema.
- **Rebuildable caches & indexes** (schema versions, filename suffixes): Invalidate and rebuild at release boundaries once source logs or ledgers cover all history. To recover records stored only in legacy caches, resolve ownership via that release's key, root-order, and path rules, then deduplicate and persist before cleanup. Filename suffixes and DB schema versions are independent.
- **Parser versions** (`parser_version`, `PARSER_VERSION`): Bump only affected parsers when output, identity keys, merge rules, or re-read fields change. Base on the last released value (starting at `1`), reuse within the cycle, and re-read existing sources while keeping recorded history. Annotate historical thresholds with their introducing release and match written values.
- **Runtime revisions & client versions** (`revision`, client log versions): Independent of migration versions; follow existing runtime rules or read directly from source.
- **Consistency & safety**: Keep readers, writers, re-read checks, and tests aligned; released version mappings are immutable. Make migrations retryable—preserve original data and report errors on unknown versions, corruption, or ambiguous ownership. Verify totals, request identities, and indexes before deleting legacy data.
- **Local dev state & tests**: Fix intermediate local dev state outside the repo (back up and test on a copy first). Verify direct upgrades, history preservation, and retries with frozen release fixtures (original schemas, fields, and keys), including deleted-source cases for caches.

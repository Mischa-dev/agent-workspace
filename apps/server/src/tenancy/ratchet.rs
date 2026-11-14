//! The unscoped call-site ratchet (H1.44, R42).
//!
//! A source scan counts, per server module directory, the call sites that
//! run a query on the pool outside a scoped transaction ([`crate::db`]):
//! a query whose executor is the pool itself, and a transaction or
//! connection taken straight from the pool. `ratchet.json` beside this file
//! holds the counts; a count above its entry fails (scope the new query),
//! and a count below it fails until the entry is lowered in the same pull
//! request, so the numbers only go down. A directory not in the file is
//! allowed none. At run time, H1.20's observe log backs the scan.
//!
//! Keys are the directories under `src/` (`mcp/`), and the files directly
//! in `src/` in four alphabetical groups (`src a-c`, `src d-l`, `src m-r`,
//! `src s-z`), one per scoping slice (H1.45 to H1.62).
//!
//! The scan sees the pool by name (`pool`, `&state.pool`, `self.pool`): a
//! helper that takes `impl PgExecutor` and is handed the pool is counted
//! where the pool is handed over only if that call is itself a query. It is
//! a ratchet, not a proof; the observe log is the proof.

use std::{collections::BTreeMap, path::Path};

/// Call sites in these are the scoping machinery itself, or test helpers.
const EXCLUDED: &[&str] = &["db.rs", "tenancy/", "test_support.rs"];

const QUERY_METHODS: &[&str] = &[
    ".fetch_one(",
    ".fetch_all(",
    ".fetch_optional(",
    ".fetch(",
    ".fetch_many(",
    ".execute(",
    ".execute_many(",
];

const POOL_METHODS: &[&str] = &[".begin()", ".begin_with(", ".acquire()"];

/// The ratchet key a source file (relative to `src/`) counts under.
pub(super) fn key(relative: &str) -> String {
    if let Some((directory, _)) = relative.split_once('/') {
        return format!("{directory}/");
    }
    let group = match relative.chars().next().unwrap_or('z') {
        'a'..='c' => "a-c",
        'd'..='l' => "d-l",
        'm'..='r' => "m-r",
        _ => "s-z",
    };
    format!("src {group}")
}

/// The source without its test module, line comments and whitespace, so a
/// call split over lines reads as one.
fn normalized(source: &str) -> String {
    let source = source
        .find("\n#[cfg(test)]\nmod tests")
        .map_or(source, |tests| &source[..tests]);
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Whether an executor or receiver expression is the pool: its last part is
/// `pool` (`pool`, `&state.pool`, `&*pool`, `self.pool()`).
fn is_pool(expression: &str) -> bool {
    let expression = expression.trim_start_matches(['&', '*']);
    let last = expression.rsplit('.').next().unwrap_or_default();
    last == "pool" || last == "pool()"
}

fn expression_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '&' | '*')
}

/// How many unscoped call sites `source` holds.
pub(super) fn count(source: &str) -> usize {
    let source = normalized(source);
    let mut found = 0;
    for method in QUERY_METHODS {
        for (index, _) in source.match_indices(method) {
            let rest = &source[index + method.len()..];
            let mut end = rest.find(|c| !expression_char(c)).unwrap_or(rest.len());
            if rest[end..].starts_with("()") {
                end += 2;
            }
            if rest[end..].starts_with([')', ',']) && is_pool(&rest[..end]) {
                found += 1;
            }
        }
    }
    for method in POOL_METHODS {
        for (index, _) in source.match_indices(method) {
            let before = &source[..index];
            let start = before
                .rfind(|c: char| !(expression_char(c) || c == '(' || c == ')'))
                .map_or(0, |position| position + 1);
            if is_pool(&before[start..]) {
                found += 1;
            }
        }
    }
    found
}

/// Counts per key over every source file under `root` (`src/`).
pub(super) fn counts(root: &Path) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("read source folder") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            if EXCLUDED.iter().any(|excluded| {
                relative == *excluded || (excluded.ends_with('/') && relative.starts_with(excluded))
            }) {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source file");
            *counts.entry(key(&relative)).or_insert(0) += count(&source);
        }
    }
    counts
}

/// What is wrong with the counts against the stored ratchet, in words.
pub(super) fn problems(
    counts: &BTreeMap<String, usize>,
    stored: &BTreeMap<String, usize>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for key in counts.keys().chain(stored.keys()) {
        let now = counts.get(key).copied().unwrap_or(0);
        let allowed = stored.get(key).copied().unwrap_or(0);
        let problem = match now.cmp(&allowed) {
            std::cmp::Ordering::Greater => format!(
                "{key}: {now} unscoped pool call sites, the ratchet allows {allowed}; \
                 run new queries through db::scoped_tx or db::scoped_one"
            ),
            std::cmp::Ordering::Less => format!(
                "{key}: {now} unscoped pool call sites; lower its entry in \
                 tenancy/ratchet.json from {allowed} to {now}"
            ),
            std::cmp::Ordering::Equal => continue,
        };
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_call_sites_only_go_down() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let counts = counts(&root);
        let stored: BTreeMap<String, usize> =
            serde_json::from_str(include_str!("ratchet.json")).expect("ratchet.json");
        let found = problems(&counts, &stored);
        assert!(
            found.is_empty(),
            "{found:#?}\n\nThe counts now, for tenancy/ratchet.json:\n{}",
            serde_json::to_string_pretty(&counts).expect("counts")
        );
    }

    #[test]
    fn the_scan_counts_pool_queries_and_not_scoped_ones() {
        let unscoped = r#"
            let rows = sqlx::query("SELECT 1").fetch_all(&state.pool).await?;
            sqlx::query("DELETE FROM x")
                .execute(
                    pool,
                )
                .await?;
            let mut transaction = state
                .pool
                .begin()
                .await?;
            let mut connection = self.pool.acquire().await?;
            sqlx::query("SELECT 2").fetch_optional(&*pool).await?;
        "#;
        assert_eq!(count(unscoped), 5);
        let scoped = r#"
            let mut transaction = db::scoped_tx(&state.pool).await?;
            sqlx::query("SELECT 1").fetch_all(&mut *transaction).await?;
            sqlx::query("SELECT 1").execute(&mut **transaction).await?;
            let nested = connection.begin().await?;
            db::scoped_one(&state.pool, async |connection| {
                sqlx::query("SELECT 1").fetch_one(connection).await
            });
            helper(executor).fetch_all(executor).await?;
            // sqlx::query("SELECT 1").fetch_all(&state.pool)
        "#;
        assert_eq!(count(scoped), 0);
        let with_tests =
            "sqlx::query(\"x\").execute(pool);\n#[cfg(test)]\nmod tests {\n    q.execute(pool);\n}";
        assert_eq!(count(with_tests), 1);
    }

    #[test]
    fn a_new_query_fails_and_so_does_a_scoped_one_left_counted() {
        let stored = BTreeMap::from([("mcp/".to_owned(), 3), ("src a-c".to_owned(), 1)]);
        let same = stored.clone();
        assert!(problems(&same, &stored).is_empty());
        let more = BTreeMap::from([("mcp/".to_owned(), 4), ("src a-c".to_owned(), 1)]);
        assert!(problems(&more, &stored)[0].starts_with("mcp/: 4"));
        let fewer = BTreeMap::from([("mcp/".to_owned(), 2), ("src a-c".to_owned(), 1)]);
        assert!(problems(&fewer, &stored)[0].contains("lower its entry"));
        // A new directory is scoped from birth.
        let new = BTreeMap::from([
            ("mcp/".to_owned(), 3),
            ("src a-c".to_owned(), 1),
            ("hosts/".to_owned(), 1),
        ]);
        assert!(problems(&new, &stored)[0].starts_with("hosts/"));
        assert_eq!(key("mcp/tools.rs"), "mcp/");
        assert_eq!(key("backup.rs"), "src a-c");
        assert_eq!(key("mcp.rs"), "src m-r");
        assert_eq!(key("work_items.rs"), "src s-z");
    }
}

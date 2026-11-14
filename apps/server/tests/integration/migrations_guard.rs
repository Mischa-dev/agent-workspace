//! Migration names: the guard that keeps branches' migrations merging in
//! either order.

/// sqlx accepts two files with the same version at compile time, and the live
/// server runs unmerged branches: a second 0046 would only show up as a
/// startup failure there ("previously applied but has been modified").
/// Numbered migrations run 1 to 0054 with no gaps. Every later one is named by
/// UTC timestamp (`20261020143000_settings.sql`), so branches that each add
/// one merge in either order: sqlx sorts them after 0054 and applies any
/// version it hasn't run, whichever merged first.
#[test]
fn migration_versions_are_unique_and_new_ones_are_timestamps() {
    const LAST_NUMBERED: i64 = 54;
    let versions: Vec<i64> = sqlx::migrate!("./migrations")
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    let numbered: Vec<i64> = versions
        .iter()
        .copied()
        .filter(|version| *version <= LAST_NUMBERED)
        .collect();
    assert_eq!(
        numbered,
        (1..=LAST_NUMBERED).collect::<Vec<_>>(),
        "migrations 0001 to 0054 must all exist, once each"
    );
    let mut timestamps = versions
        .iter()
        .copied()
        .filter(|version| *version > LAST_NUMBERED)
        .collect::<Vec<_>>();
    for version in &timestamps {
        let text = version.to_string();
        let field = |at: usize| text[at..at + 2].parse::<u32>().expect("digits");
        assert!(
            text.len() == 14
                && (2026..2100).contains(&(version / 10_000_000_000))
                && (1..=12).contains(&field(4))
                && (1..=31).contains(&field(6))
                && field(8) < 24
                && field(10) < 60
                && field(12) < 60,
            "migration {version}: name new migrations by UTC timestamp, such as 20261020143000_settings.sql"
        );
    }
    let count = timestamps.len();
    timestamps.dedup();
    assert_eq!(timestamps.len(), count, "two migrations share a timestamp");
}

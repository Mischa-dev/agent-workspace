//! Guard 1 (M1 section 6): no module id matched by hand.
//!
//! Code outside the registry (`modules/registry.rs`, `modules/state.rs` and
//! each module's own `modules/core/<id>/` folder) shouldn't decide things by
//! naming a core module in a string: `contains("tasks")`, `== "pages"`,
//! `manifest::get("runs")`, `matches!(…, "channels")`, `module_off(…,
//! "Pages")`. Register a `ModuleFilter` or use the operation's module
//! instead. Today's sites are counted per file in `hand_matched.allowlist`;
//! a file's count may only fall, and a file not listed may have none.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::manifest::core;

const ALLOWLIST: &str = include_str!("hand_matched.allowlist");

fn rust_files(folder: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

/// The hand matches on one line.
pub(super) fn hand_matches(line: &str) -> usize {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return 0;
    }
    let mut found = 0;
    for module in core() {
        let id = format!("\"{}\"", module.id);
        let name = format!("\"{}\"", module.name);
        for pattern in [
            format!("contains({id})"),
            format!("== {id}"),
            format!("{id} =="),
            format!("get({id})"),
        ] {
            found += line.matches(pattern.as_str()).count();
        }
        if line.contains("with(") && line.contains(&format!(", {id})")) {
            found += 1;
        }
        if line.contains("matches!(") && line.contains(id.as_str()) {
            found += 1;
        }
        if line.contains("module_off(") && line.contains(name.as_str()) {
            found += 1;
        }
    }
    found
}

/// A file without its inline test module: tests may name modules, since
/// they check what a module does. Only an inline `mod … {` marked
/// `#[cfg(test)]` ends the code; a `#[cfg(test)] mod x;` near the top
/// (an outlined test file) doesn't hide the rest of the file.
pub(super) fn code_of(text: &str) -> &str {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines.peek().is_some_and(|next| {
                let next = next.trim();
                (next.starts_with("mod ")
                    || next.starts_with("pub mod ")
                    || next.starts_with("pub(crate) mod "))
                    && next.ends_with('{')
            })
        {
            return &text[..offset];
        }
        offset += line.len();
    }
    text
}

fn counts() -> BTreeMap<String, usize> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("src");
    let mut files = Vec::new();
    rust_files(&source, &mut files);
    files.sort();
    let mut counts = BTreeMap::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if relative == "src/modules/registry.rs"
            || relative == "src/modules/state.rs"
            || relative == "src/modules/guards.rs"
            || relative.starts_with("src/modules/core/")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let found = code_of(&text).lines().map(hand_matches).sum::<usize>();
        if found > 0 {
            counts.insert(relative, found);
        }
    }
    counts
}

fn allowed() -> BTreeMap<String, usize> {
    ALLOWLIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (file, count) = line.rsplit_once(':')?;
            Some((file.trim().to_owned(), count.trim().parse().ok()?))
        })
        .collect()
}

#[test]
fn module_ids_are_matched_only_in_the_registry() {
    let allowed = allowed();
    let mut problems = Vec::new();
    let mut shrunk = Vec::new();
    let found = counts();
    for (file, count) in &found {
        let limit = allowed.get(file).copied().unwrap_or(0);
        if *count > limit {
            problems.push(format!(
                "{file}: {count} module ids matched by hand (allowed {limit})"
            ));
        } else if *count < limit {
            shrunk.push(format!("{file}: {count}"));
        }
    }
    for (file, limit) in &allowed {
        if !found.contains_key(file) {
            shrunk.push(format!("{file}: 0 (was {limit})"));
        }
    }
    if !shrunk.is_empty() {
        eprintln!("hand_matched.allowlist can shrink:\n{}", shrunk.join("\n"));
    }
    assert!(
        problems.is_empty(),
        "A module is named in a string to decide something. Register a ModuleFilter, or use \
         the operation's module, instead of matching its id by hand:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_guard_sees_a_hand_match() {
    assert_eq!(hand_matches(r#"if modules.contains("tasks") {"#), 1);
    assert_eq!(hand_matches(r#"let on = module == "pages";"#), 1);
    assert_eq!(hand_matches(r#"// modules.contains("tasks")"#), 0);
    assert_eq!(hand_matches(r#"let name = "tasks";"#), 0);
}

#[test]
fn only_an_inline_test_module_ends_the_code() {
    let outlined = "#[cfg(test)]\nmod words;\nfn a() {}\n#[cfg(test)]\nmod tests {\n}\n";
    assert_eq!(code_of(outlined), "#[cfg(test)]\nmod words;\nfn a() {}\n");
    assert_eq!(code_of("fn a() {}\n"), "fn a() {}\n");
}

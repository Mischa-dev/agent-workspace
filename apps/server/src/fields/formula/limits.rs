//! What a formula may be and do (T1.17, plan T1 section 3.2 "Limits"), so a
//! formula someone writes can never make a record write slow or large.

/// Characters a formula's source may have, as written.
pub const MAX_SOURCE_CHARACTERS: usize = 2_000;
/// How deeply operations and calls may nest.
pub const MAX_DEPTH: usize = 32;
/// Field references one formula may make (each mention counts).
pub const MAX_REFERENCES: usize = 20;
/// Evaluation steps one record's formula may take.
pub const MAX_STEPS: u32 = 10_000;
/// Characters a text result may have.
pub const MAX_TEXT_RESULT_CHARACTERS: usize = 5_000;
/// Characters any text may grow to while a formula runs.
pub const MAX_TEXT_CHARACTERS: usize = 20_000;
/// Formula fields one collection may have, archived ones aside.
pub const MAX_FORMULA_FIELDS: usize = 10;
/// Bytes a record's computed values may take, as compact JSON. Below the
/// column's 64 KB check, which counts PostgreSQL's spaced-out text.
pub const MAX_COMPUTED_BYTES: usize = 48 * 1024;
/// Records one recalculation job run works through before it hands on.
pub const RECOMPUTE_BATCH: i64 = 500;

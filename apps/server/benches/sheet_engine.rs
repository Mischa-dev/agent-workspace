//! How long a sheet write's recalculation takes as a sheet grows (C1.21).
//!
//! Every write to a sheet rebuilds its workbook in the formula engine,
//! applies the change and recalculates everything, so the time grows with
//! the filled cells. This measures that for sheets of 25,000 to 200,000
//! cells, a fifth of them formulas, with one cell changed:
//!
//! ```text
//! cargo bench -p agent-workspace-server --bench sheet_engine
//! ```
//!
//! Run it on a CI-sized machine. The numbers and the limit they justify are
//! in `docs/plans/sheets.md`.

use std::time::{Duration, Instant};

use agent_workspace_server::sheets::engine::{FormulaEngine, IronCalc, Operation, TabInput};

const COLUMNS: i32 = 20;
const RUNS: usize = 5;

/// A tab of `cells` filled cells, `COLUMNS` wide: numbers, and in every
/// fifth column a formula over the row's numbers before it.
fn tab(cells: usize) -> TabInput {
    let rows = (cells as i32 + COLUMNS - 1) / COLUMNS;
    let mut filled = Vec::with_capacity(cells);
    'rows: for row in 1..=rows {
        for column in 1..=COLUMNS {
            if filled.len() == cells {
                break 'rows;
            }
            let input = if column % 5 == 0 {
                let first = column_name(column - 4);
                let last = column_name(column - 1);
                format!("=SUM({first}{row}:{last}{row})*2")
            } else {
                (row * column).to_string()
            };
            filled.push((row, column, input));
        }
    }
    TabInput {
        name: "Sheet1".to_owned(),
        cells: filled,
        looks: Vec::new(),
    }
}

fn column_name(column: i32) -> String {
    let mut name = String::new();
    let mut left = column;
    while left > 0 {
        let digit = (left - 1) % 26;
        name.insert(0, char::from(b'A' + digit as u8));
        left = (left - 1) / 26;
    }
    name
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn main() {
    let engine = IronCalc;
    println!("cells    median write   slowest");
    for cells in [25_000, 50_000, 100_000, 200_000] {
        let tabs = vec![tab(cells)];
        let change = vec![Operation::Set {
            tab: 0,
            row: 1,
            column: 1,
            input: "42".to_owned(),
        }];
        let mut times = Vec::with_capacity(RUNS);
        for _ in 0..RUNS {
            let started = Instant::now();
            let output = engine.apply(&tabs, &change).expect("the write applies");
            times.push(started.elapsed());
            assert_eq!(output[0].cells.len(), cells);
        }
        let slowest = *times.iter().max().expect("runs");
        println!(
            "{cells:>7}  {:>10.0} ms  {:>7.0} ms",
            median(times).as_secs_f64() * 1000.0,
            slowest.as_secs_f64() * 1000.0
        );
    }
}

//! A1 names for cells and ranges: `B12`, `A1:D20`, whole columns (`B:D`)
//! and whole rows (`3:7`). Rows and columns count from 1.

/// A column's letters: 1 is A, 27 is AA.
pub fn column_name(mut column: i32) -> String {
    let mut letters = Vec::new();
    while column > 0 {
        let remainder = (column - 1) % 26;
        letters.push(b'A' + remainder as u8);
        column = (column - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_default()
}

/// A column's number from its letters, ignoring case.
pub fn column_number(letters: &str) -> Option<i32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    letters.chars().try_fold(0_i32, |total, letter| {
        letter
            .is_ascii_alphabetic()
            .then(|| total * 26 + (letter.to_ascii_uppercase() as i32 - 'A' as i32 + 1))
    })
}

/// A cell such as `B12` (`$` signs are allowed): (row, column).
pub fn cell(value: &str) -> Option<(i32, i32)> {
    let value = value.trim().replace('$', "");
    let split = value.find(|character: char| character.is_ascii_digit())?;
    let (letters, digits) = value.split_at(split);
    let column = column_number(letters)?;
    let row = digits.parse::<i32>().ok().filter(|row| *row > 0)?;
    Some((row, column))
}

/// A block of cells, both ends inclusive: rows then columns. Open ends
/// (a whole column or row) run to `max_rows` or `max_columns`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub rows: (i32, i32),
    pub columns: (i32, i32),
}

impl Range {
    pub fn name(&self) -> String {
        format!(
            "{}{}:{}{}",
            column_name(self.columns.0),
            self.rows.0,
            column_name(self.columns.1),
            self.rows.1
        )
    }

    pub fn cells(&self) -> i64 {
        i64::from(self.rows.1 - self.rows.0 + 1) * i64::from(self.columns.1 - self.columns.0 + 1)
    }
}

/// `A1:D20`, `B3` (one cell), `B:D` (whole columns) or `3:7` (whole rows).
pub fn range(value: &str, max_rows: i32, max_columns: i32) -> Option<Range> {
    let value = value.trim().replace('$', "");
    let (start, end) = match value.split_once(':') {
        Some((start, end)) => (start.trim().to_owned(), end.trim().to_owned()),
        None => (value.clone(), value.clone()),
    };
    let ordered = |a: i32, b: i32| if a <= b { (a, b) } else { (b, a) };
    if let (Some((row_a, column_a)), Some((row_b, column_b))) = (cell(&start), cell(&end)) {
        return Some(Range {
            rows: ordered(row_a, row_b),
            columns: ordered(column_a, column_b),
        });
    }
    if let (Some(a), Some(b)) = (column_number(&start), column_number(&end)) {
        return Some(Range {
            rows: (1, max_rows),
            columns: ordered(a, b),
        });
    }
    if let (Ok(a), Ok(b)) = (start.parse::<i32>(), end.parse::<i32>())
        && a > 0
        && b > 0
    {
        return Some(Range {
            rows: ordered(a, b),
            columns: (1, max_columns),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_cells() {
        assert_eq!(column_name(1), "A");
        assert_eq!(column_name(26), "Z");
        assert_eq!(column_name(27), "AA");
        assert_eq!(column_name(200), "GR");
        assert_eq!(column_number("gr"), Some(200));
        assert_eq!(cell("$B$12"), Some((12, 2)));
        assert_eq!(cell("B0"), None);
        assert_eq!(cell("12"), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(
            range("D20:A1", 100, 10),
            Some(Range {
                rows: (1, 20),
                columns: (1, 4)
            })
        );
        assert_eq!(
            range("B:C", 100, 10),
            Some(Range {
                rows: (1, 100),
                columns: (2, 3)
            })
        );
        assert_eq!(
            range("3:4", 100, 10),
            Some(Range {
                rows: (3, 4),
                columns: (1, 10)
            })
        );
        assert_eq!(range("B3", 100, 10).expect("ok").cells(), 1);
        assert_eq!(range("nonsense!", 100, 10), None);
        assert_eq!(range("A1:D20", 100, 10).expect("ok").name(), "A1:D20");
    }
}

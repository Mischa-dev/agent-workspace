//! The functions a formula can call (T1.17's first set). Names follow
//! spreadsheets, so people who know Sheets know them. Each has a type check
//! (`check`, run when the formula is saved) and a body (`call`, run per
//! record). Arguments are evaluated lazily where the function decides which
//! it needs (IF, IFS, SWITCH, AND, OR, COALESCE).

use std::collections::HashSet;

use chrono::{Datelike, Duration, Months, NaiveDate};

use super::{
    eval::{Run, compare, number},
    parse::Expr,
    value::{Ty, V, join, short},
};

/// Every function, for messages and the web app's autocomplete.
pub const NAMES: [&str; 45] = [
    "IF",
    "IFS",
    "SWITCH",
    "AND",
    "OR",
    "NOT",
    "ISBLANK",
    "COALESCE",
    "SUM",
    "MIN",
    "MAX",
    "AVG",
    "ROUND",
    "FLOOR",
    "CEIL",
    "ABS",
    "MOD",
    "POW",
    "CONCAT",
    "LEFT",
    "RIGHT",
    "LEN",
    "LOWER",
    "UPPER",
    "TRIM",
    "CONTAINS",
    "REPLACE",
    "TEXT",
    "NUMBER",
    "TODAY",
    "NOW",
    "DATE",
    "DATEADD",
    "DATEDIFF",
    "YEAR",
    "MONTH",
    "DAY",
    "WEEKDAY",
    "FORMATDATE",
    "COUNT",
    "JOIN",
    "FIRST",
    "LAST",
    "UNIQUE",
    "AVERAGE",
];

/// Functions whose value changes with the time, not only the record.
pub const VOLATILE: [&str; 2] = ["TODAY", "NOW"];

/// How many arguments a function takes: at least, at most (None: any).
fn arity(name: &str) -> Option<(usize, Option<usize>)> {
    Some(match name {
        "IF" => (2, Some(3)),
        "IFS" => (2, None),
        "SWITCH" => (3, None),
        "AND" | "OR" | "COALESCE" | "SUM" | "MIN" | "MAX" | "AVG" | "AVERAGE" | "CONCAT"
        | "COUNT" => (1, None),
        "NOT" | "ISBLANK" | "ABS" | "CEIL" | "LEN" | "LOWER" | "UPPER" | "TRIM" | "TEXT"
        | "NUMBER" | "YEAR" | "MONTH" | "DAY" | "WEEKDAY" | "FIRST" | "LAST" | "UNIQUE" => {
            (1, Some(1))
        }
        "ROUND" | "FLOOR" | "LEFT" | "RIGHT" => (1, Some(2)),
        "MOD" | "POW" | "CONTAINS" | "FORMATDATE" => (2, Some(2)),
        "JOIN" => (1, Some(2)),
        "REPLACE" => (3, Some(3)),
        "TODAY" | "NOW" => (0, Some(0)),
        "DATE" => (1, Some(3)),
        "DATEADD" | "DATEDIFF" => (2, Some(3)),
        _ => return None,
    })
}

fn wants(name: &str, position: usize, ty: Ty, want: Ty) -> Result<(), String> {
    if ty.fits(want) {
        Ok(())
    } else {
        Err(format!(
            "{name}'s part {} should be {}, not {}.",
            position + 1,
            want.name(),
            ty.name()
        ))
    }
}

/// The type a call gives, or why it doesn't add up.
pub fn check(name: &str, args: &[Ty]) -> Result<Ty, String> {
    let Some((min, max)) = arity(name) else {
        return Err(format!(
            "There's no function called {name}. Functions: {}.",
            NAMES.join(", ")
        ));
    };
    if args.len() < min || max.is_some_and(|max| args.len() > max) {
        return Err(match (min, max) {
            (0, Some(0)) => format!("{name}() takes nothing between its brackets."),
            (min, Some(max)) if min == max => format!("{name} takes {min} part(s)."),
            (min, Some(max)) => format!("{name} takes {min} to {max} parts."),
            (min, None) => format!("{name} takes at least {min} part(s)."),
        });
    }
    let numbers = |args: &[Ty]| -> Result<(), String> {
        for (index, ty) in args.iter().enumerate() {
            if !(ty.fits(Ty::Number) || *ty == Ty::List || *ty == Ty::Text) {
                return Err(format!(
                    "{name}'s part {} should be a number or a list, not {}.",
                    index + 1,
                    ty.name()
                ));
            }
        }
        Ok(())
    };
    let fold =
        |types: &mut dyn Iterator<Item = Ty>| -> Ty { types.reduce(Ty::common).unwrap_or(Ty::Any) };
    Ok(match name {
        // Any value can be a condition: blank, 0 and "" are false.
        "IF" => match args.get(2) {
            Some(otherwise) => args[1].common(*otherwise),
            None => args[1],
        },
        "IFS" => {
            if args.len() % 2 != 0 {
                return Err(
                    "IFS takes pairs: a condition, then its value, for each case.".to_owned(),
                );
            }
            fold(&mut args.iter().skip(1).step_by(2).copied())
        }
        "SWITCH" => {
            let cases = &args[1..];
            let values = cases
                .chunks(2)
                .map(|pair| if pair.len() == 2 { pair[1] } else { pair[0] });
            fold(&mut values.into_iter())
        }
        "AND" | "OR" | "NOT" | "ISBLANK" => Ty::Boolean,
        "COALESCE" => fold(&mut args.iter().copied()),
        "SUM" | "AVG" | "AVERAGE" => {
            numbers(args)?;
            Ty::Number
        }
        "MIN" | "MAX" => {
            if args.iter().all(|ty| *ty == Ty::Date) {
                Ty::Date
            } else {
                numbers(args)?;
                Ty::Number
            }
        }
        "ROUND" | "FLOOR" | "CEIL" | "ABS" | "MOD" | "POW" => {
            for (index, ty) in args.iter().enumerate() {
                wants(name, index, *ty, Ty::Number)
                    .or_else(|error| if *ty == Ty::Text { Ok(()) } else { Err(error) })?;
            }
            Ty::Number
        }
        "CONCAT" | "LOWER" | "UPPER" | "TRIM" | "TEXT" | "REPLACE" => Ty::Text,
        "LEFT" | "RIGHT" => {
            if let Some(count) = args.get(1) {
                wants(name, 1, *count, Ty::Number)?;
            }
            Ty::Text
        }
        "LEN" | "COUNT" | "NUMBER" => Ty::Number,
        "CONTAINS" => Ty::Boolean,
        "TODAY" | "NOW" => Ty::Date,
        "DATE" => {
            if args.len() == 1 {
                wants(name, 0, args[0], Ty::Text).or_else(|_| wants(name, 0, args[0], Ty::Date))?;
            } else if args.len() == 3 {
                for (index, ty) in args.iter().enumerate() {
                    wants(name, index, *ty, Ty::Number)?;
                }
            } else {
                return Err(
                    "DATE takes a date as \"YYYY-MM-DD\", or a year, a month and a day.".to_owned(),
                );
            }
            Ty::Date
        }
        "DATEADD" => {
            date_like(name, 0, args[0])?;
            wants(name, 1, args[1], Ty::Number)?;
            if let Some(unit) = args.get(2) {
                wants(name, 2, *unit, Ty::Text)?;
            }
            Ty::Date
        }
        "DATEDIFF" => {
            date_like(name, 0, args[0])?;
            date_like(name, 1, args[1])?;
            if let Some(unit) = args.get(2) {
                wants(name, 2, *unit, Ty::Text)?;
            }
            Ty::Number
        }
        "YEAR" | "MONTH" | "DAY" | "WEEKDAY" => {
            date_like(name, 0, args[0])?;
            Ty::Number
        }
        "FORMATDATE" => {
            date_like(name, 0, args[0])?;
            wants(name, 1, args[1], Ty::Text)?;
            Ty::Text
        }
        "JOIN" => Ty::Text,
        "FIRST" | "LAST" => Ty::Any,
        "UNIQUE" => Ty::List,
        _ => Ty::Any,
    })
}

fn date_like(name: &str, position: usize, ty: Ty) -> Result<(), String> {
    if ty.fits(Ty::Date) || ty == Ty::Text {
        Ok(())
    } else {
        Err(format!(
            "{name}'s part {} should be a date, not {}.",
            position + 1,
            ty.name()
        ))
    }
}

macro_rules! value {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
}

/// Every argument's value, stopping at the first error.
fn all(run: &mut Run<'_>, args: &[Expr]) -> Result<Vec<V>, V> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        let value = run.eval(arg);
        if value.is_err() {
            return Err(value);
        }
        values.push(value);
    }
    Ok(values)
}

/// The numbers among the arguments, lists flattened, blanks left out.
fn numbers(run: &mut Run<'_>, args: &[Expr]) -> Result<Vec<f64>, V> {
    let mut found = Vec::new();
    for value in all(run, args)? {
        let items = value.items();
        run.spend(items.len())?;
        for item in items {
            if item.is_blank() {
                continue;
            }
            found.push(item.to_num()?);
        }
    }
    Ok(found)
}

fn round_to(value: f64, digits: i32) -> f64 {
    let digits = digits.clamp(-15, 15);
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

/// Runs a call. The name was checked when the formula was saved; an unknown
/// one (a formula saved by a newer server) is an error value.
pub fn call(run: &mut Run<'_>, name: &str, args: &[Expr]) -> V {
    match name {
        "IF" => {
            let condition = value!(run.eval(&args[0]).to_bool());
            if condition {
                run.eval(&args[1])
            } else {
                args.get(2)
                    .map_or(V::Blank, |otherwise| run.eval(otherwise))
            }
        }
        "IFS" => {
            for pair in args.chunks(2) {
                if pair.len() == 2 && value!(run.eval(&pair[0]).to_bool()) {
                    return run.eval(&pair[1]);
                }
            }
            V::Blank
        }
        "SWITCH" => {
            let subject = run.eval(&args[0]);
            if subject.is_err() {
                return subject;
            }
            let cases = &args[1..];
            for pair in cases.chunks(2) {
                if pair.len() == 1 {
                    return run.eval(&pair[0]);
                }
                let case = run.eval(&pair[0]);
                if case.is_err() {
                    return case;
                }
                if compare(&subject, &case).is_ok_and(|ordering| ordering.is_eq()) {
                    return run.eval(&pair[1]);
                }
            }
            V::Blank
        }
        "AND" | "OR" => {
            let want = name == "OR";
            for arg in args {
                if value!(run.eval(arg).to_bool()) == want {
                    return V::Bool(want);
                }
            }
            V::Bool(!want)
        }
        "NOT" => V::Bool(!value!(run.eval(&args[0]).to_bool())),
        "ISBLANK" => {
            let value = run.eval(&args[0]);
            if value.is_err() {
                return value;
            }
            V::Bool(value.is_blank())
        }
        "COALESCE" => {
            for arg in args {
                let value = run.eval(arg);
                if !value.is_blank() {
                    return value;
                }
            }
            V::Blank
        }
        "SUM" => number(value!(numbers(run, args)).iter().sum()),
        "AVG" | "AVERAGE" => {
            let found = value!(numbers(run, args));
            if found.is_empty() {
                V::Blank
            } else {
                number(found.iter().sum::<f64>() / found.len() as f64)
            }
        }
        "MIN" | "MAX" => {
            let values = value!(all(run, args));
            let items = values
                .iter()
                .flat_map(V::items)
                .filter(|item| !item.is_blank());
            let mut best: Option<V> = None;
            for item in items {
                if matches!(item, V::Text(_)) {
                    value!(item.to_num());
                }
                best = Some(match best {
                    None => item,
                    Some(current) => {
                        let ordering = value!(compare(&item, &current));
                        if (name == "MIN" && ordering.is_lt())
                            || (name == "MAX" && ordering.is_gt())
                        {
                            item
                        } else {
                            current
                        }
                    }
                });
            }
            match best {
                Some(V::Text(text)) => V::Text(text)
                    .to_num()
                    .map(number)
                    .unwrap_or_else(|error| error),
                Some(value) => value,
                None => V::Blank,
            }
        }
        "ROUND" | "FLOOR" | "CEIL" => {
            let value = value!(run.eval(&args[0]).to_num());
            let digits = match args.get(1) {
                Some(digits) => value!(run.eval(digits).to_num()) as i32,
                None => 0,
            };
            let scale = 10f64.powi(digits.clamp(-15, 15));
            number(match name {
                "ROUND" => round_to(value, digits),
                "FLOOR" => (value * scale).floor() / scale,
                _ => (value * scale).ceil() / scale,
            })
        }
        "ABS" => number(value!(run.eval(&args[0]).to_num()).abs()),
        "MOD" => {
            let a = value!(run.eval(&args[0]).to_num());
            let b = value!(run.eval(&args[1]).to_num());
            if b == 0.0 {
                return V::Err("Divide by zero".to_owned());
            }
            // The sign follows the divisor, as in spreadsheets.
            number(a - b * (a / b).floor())
        }
        "POW" => {
            let a = value!(run.eval(&args[0]).to_num());
            let b = value!(run.eval(&args[1]).to_num());
            number(a.powf(b))
        }
        "CONCAT" => {
            let mut text = String::new();
            for value in value!(all(run, args)) {
                match join(text, &value!(value.to_text())) {
                    V::Text(joined) => text = joined,
                    error => return error,
                }
            }
            V::Text(text)
        }
        "LEFT" | "RIGHT" => {
            let text = value!(run.eval(&args[0]).to_text());
            let count = match args.get(1) {
                Some(count) => value!(run.eval(count).to_num()).max(0.0) as usize,
                None => 1,
            };
            let characters = text.chars().collect::<Vec<_>>();
            let count = count.min(characters.len());
            V::Text(if name == "LEFT" {
                characters[..count].iter().collect()
            } else {
                characters[characters.len() - count..].iter().collect()
            })
        }
        "LEN" => V::Num(value!(run.eval(&args[0]).to_text()).chars().count() as f64),
        "LOWER" => V::Text(value!(run.eval(&args[0]).to_text()).to_lowercase()),
        "UPPER" => V::text(value!(run.eval(&args[0]).to_text()).to_uppercase()),
        "TRIM" => V::Text(
            value!(run.eval(&args[0]).to_text())
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "CONTAINS" => {
            let haystack = run.eval(&args[0]);
            let needle = value!(run.eval(&args[1]).to_text()).to_lowercase();
            if let V::List(items) = &haystack {
                value!(run.spend(items.len()));
                for item in items {
                    if value!(item.to_text()).to_lowercase() == needle {
                        return V::Bool(true);
                    }
                }
                return V::Bool(false);
            }
            V::Bool(value!(haystack.to_text()).to_lowercase().contains(&needle))
        }
        "REPLACE" => {
            let text = value!(run.eval(&args[0]).to_text());
            let find = value!(run.eval(&args[1]).to_text());
            let with = value!(run.eval(&args[2]).to_text());
            if find.is_empty() {
                return V::Text(text);
            }
            V::text(text.replace(&find, &with))
        }
        "TEXT" => V::Text(value!(run.eval(&args[0]).to_text())),
        "NUMBER" => {
            let value = run.eval(&args[0]);
            if value.is_blank() {
                return V::Blank;
            }
            number(value!(value.to_num()))
        }
        "TODAY" => V::Date(run.now.date_naive()),
        "NOW" => V::Time(run.now),
        "DATE" => {
            if args.len() == 1 {
                let value = run.eval(&args[0]);
                return match value!(value.to_date()) {
                    Some(date) => V::Date(date),
                    None => V::Blank,
                };
            }
            let values = value!(all(run, args));
            let parts = value!(values.iter().map(V::to_num).collect::<Result<Vec<_>, _>>());
            let (year, month, day) = (parts[0] as i32, parts[1] as i64, parts[2] as i64);
            // Months and days past their end roll over, as in spreadsheets.
            let Some(start) = NaiveDate::from_ymd_opt(year, 1, 1) else {
                return V::Err(format!("{year} isn't a year a date can have."));
            };
            let with_months = if month >= 1 {
                start.checked_add_months(Months::new((month - 1).min(12_000) as u32))
            } else {
                start.checked_sub_months(Months::new((1 - month).min(12_000) as u32))
            };
            match with_months.and_then(|date| date.checked_add_signed(Duration::days(day - 1))) {
                Some(date) => V::Date(date),
                None => V::Err("That date is out of range.".to_owned()),
            }
        }
        "DATEADD" => {
            let base = run.eval(&args[0]);
            let amount = value!(run.eval(&args[1]).to_num());
            let unit = match args.get(2) {
                Some(unit) => value!(run.eval(unit).to_text()),
                None => "days".to_owned(),
            };
            date_add(&base, amount, &unit)
        }
        "DATEDIFF" => {
            let later = run.eval(&args[0]);
            let earlier = run.eval(&args[1]);
            let unit = match args.get(2) {
                Some(unit) => value!(run.eval(unit).to_text()),
                None => "days".to_owned(),
            };
            date_diff(&later, &earlier, &unit)
        }
        "YEAR" | "MONTH" | "DAY" | "WEEKDAY" => {
            let Some(date) = value!(run.eval(&args[0]).to_date()) else {
                return V::Blank;
            };
            V::Num(match name {
                "YEAR" => f64::from(date.year()),
                "MONTH" => f64::from(date.month()),
                "DAY" => f64::from(date.day()),
                // 1 is Monday, 7 Sunday.
                _ => f64::from(date.weekday().number_from_monday()),
            })
        }
        "FORMATDATE" => {
            let value = run.eval(&args[0]);
            let pattern = value!(run.eval(&args[1]).to_text());
            format_date(&value, &pattern)
        }
        "COUNT" => {
            let mut count = 0usize;
            for value in value!(all(run, args)) {
                count += value.items().iter().filter(|item| !item.is_blank()).count();
            }
            V::Num(count as f64)
        }
        "JOIN" => {
            let list = run.eval(&args[0]);
            let separator = match args.get(1) {
                Some(separator) => value!(run.eval(separator).to_text()),
                None => ", ".to_owned(),
            };
            let items = list.items();
            value!(run.spend(items.len()));
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter().filter(|item| !item.is_blank()) {
                parts.push(value!(item.to_text()));
            }
            V::text(parts.join(&separator))
        }
        "FIRST" | "LAST" => {
            let value = run.eval(&args[0]);
            if value.is_err() {
                return value;
            }
            let items = value.items();
            let picked = if name == "FIRST" {
                items.into_iter().next()
            } else {
                items.into_iter().last()
            };
            picked.unwrap_or(V::Blank)
        }
        "UNIQUE" => {
            let value = run.eval(&args[0]);
            if value.is_err() {
                return value;
            }
            let items = value.items();
            value!(run.spend(items.len()));
            let mut seen = HashSet::new();
            let mut kept = Vec::new();
            for item in items {
                let key = value!(item.to_text()).to_lowercase();
                if seen.insert(key) {
                    kept.push(item);
                }
            }
            V::List(kept)
        }
        other => V::Err(format!("{other} isn't a function this server knows.")),
    }
}

fn unit_of(unit: &str) -> Option<&'static str> {
    Some(
        match unit.trim().to_ascii_lowercase().trim_end_matches('s') {
            "day" | "d" => "days",
            "week" | "w" => "weeks",
            "month" | "m" => "months",
            "year" | "y" => "years",
            "hour" | "h" => "hours",
            "minute" | "min" => "minutes",
            _ => return None,
        },
    )
}

fn bad_unit(unit: &str) -> V {
    V::Err(format!(
        "\"{}\" isn't a unit: use days, weeks, months, years, hours or minutes.",
        short(unit)
    ))
}

fn date_add(base: &V, amount: f64, unit: &str) -> V {
    let Some(unit) = unit_of(unit) else {
        return bad_unit(unit);
    };
    if base.is_err() {
        return base.clone();
    }
    let whole = amount.trunc() as i64;
    if let V::Time(time) = base {
        let moved = match unit {
            "minutes" => time.checked_add_signed(Duration::minutes(whole)),
            "hours" => time.checked_add_signed(Duration::hours(whole)),
            "days" => time.checked_add_signed(Duration::days(whole)),
            "weeks" => time.checked_add_signed(Duration::weeks(whole)),
            _ => None,
        };
        if let Some(moved) = moved {
            return V::Time(moved);
        }
    }
    let Some(date) = value!(base.to_date()) else {
        return V::Blank;
    };
    let months = |count: i64| {
        if count >= 0 {
            date.checked_add_months(Months::new(count.min(120_000) as u32))
        } else {
            date.checked_sub_months(Months::new((-count).min(120_000) as u32))
        }
    };
    let moved = match unit {
        "days" => date.checked_add_signed(Duration::days(whole)),
        "weeks" => date.checked_add_signed(Duration::weeks(whole)),
        "months" => months(whole),
        "years" => months(whole.saturating_mul(12)),
        // A day moved by hours or minutes is a moment.
        _ => {
            let start = date.and_hms_opt(0, 0, 0).map(|time| time.and_utc());
            let moved = start.and_then(|start| {
                start.checked_add_signed(if unit == "hours" {
                    Duration::hours(whole)
                } else {
                    Duration::minutes(whole)
                })
            });
            return moved.map_or_else(|| V::Err("That date is out of range.".to_owned()), V::Time);
        }
    };
    moved.map_or_else(|| V::Err("That date is out of range.".to_owned()), V::Date)
}

fn date_diff(later: &V, earlier: &V, unit: &str) -> V {
    let Some(unit) = unit_of(unit) else {
        return bad_unit(unit);
    };
    for value in [later, earlier] {
        if value.is_err() {
            return value.clone();
        }
        if value.is_blank() {
            return V::Blank;
        }
    }
    let moment = |value: &V| -> Result<chrono::DateTime<chrono::Utc>, V> {
        match value {
            V::Time(time) => Ok(*time),
            other => other
                .to_date()?
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|time| time.and_utc())
                .ok_or(V::Blank),
        }
    };
    let (a, b) = match (moment(later), moment(earlier)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(error), _) | (_, Err(error)) => return error,
    };
    let span = a - b;
    V::Num(match unit {
        "minutes" => span.num_minutes() as f64,
        "hours" => span.num_hours() as f64,
        "days" => span.num_days() as f64,
        "weeks" => (span.num_days() / 7) as f64,
        _ => {
            let (from, to) = (b.date_naive(), a.date_naive());
            let mut months = i64::from(to.year() - from.year()) * 12 + i64::from(to.month())
                - i64::from(from.month());
            // A month only counts once its day is reached.
            if months > 0 && to.day() < from.day() {
                months -= 1;
            } else if months < 0 && to.day() > from.day() {
                months += 1;
            }
            if unit == "years" {
                (months / 12) as f64
            } else {
                months as f64
            }
        }
    })
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// A date written by a pattern: YYYY, YY, MMMM (January), MMM (Jan), MM,
/// M, DD, D, dddd (Monday), ddd (Mon), and for a moment HH and mm.
fn format_date(value: &V, pattern: &str) -> V {
    if value.is_err() {
        return value.clone();
    }
    let (date, time) = match value {
        V::Time(time) => (time.date_naive(), Some(*time)),
        other => match value!(other.to_date()) {
            Some(date) => (date, None),
            None => return V::Blank,
        },
    };
    let tokens: [(&str, String); 12] = [
        ("YYYY", format!("{:04}", date.year())),
        ("YY", format!("{:02}", date.year().rem_euclid(100))),
        ("MMMM", MONTH_NAMES[date.month0() as usize].to_owned()),
        ("MMM", MONTH_NAMES[date.month0() as usize][..3].to_owned()),
        ("MM", format!("{:02}", date.month())),
        ("M", date.month().to_string()),
        ("DD", format!("{:02}", date.day())),
        ("D", date.day().to_string()),
        (
            "dddd",
            DAY_NAMES[date.weekday().num_days_from_monday() as usize].to_owned(),
        ),
        (
            "ddd",
            DAY_NAMES[date.weekday().num_days_from_monday() as usize][..3].to_owned(),
        ),
        (
            "HH",
            time.map_or_else(|| "00".to_owned(), |time| time.format("%H").to_string()),
        ),
        (
            "mm",
            time.map_or_else(|| "00".to_owned(), |time| time.format("%M").to_string()),
        ),
    ];
    let mut out = String::new();
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        for (token, text) in &tokens {
            if let Some(after) = rest.strip_prefix(token) {
                out.push_str(text);
                rest = after;
                continue 'outer;
            }
        }
        let mut characters = rest.chars();
        if let Some(character) = characters.next() {
            out.push(character);
        }
        rest = characters.as_str();
    }
    V::text(out)
}

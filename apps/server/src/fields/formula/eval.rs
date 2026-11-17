//! Checking a formula's types before it's saved, and running it on a record.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};

use super::{
    functions,
    limits::MAX_STEPS,
    parse::{BinOp, Expr, FieldRef},
    value::{Ty, V, join},
};

/// Where a running formula reads its fields.
pub trait Fields {
    fn value(&self, reference: FieldRef) -> V;
}

/// What a formula runs with: the record's values, the time, and its budget
/// of steps.
pub struct Run<'a> {
    pub fields: &'a dyn Fields,
    pub now: DateTime<Utc>,
    steps: u32,
}

impl<'a> Run<'a> {
    pub fn new(fields: &'a dyn Fields, now: DateTime<Utc>) -> Self {
        Self {
            fields,
            now,
            steps: 0,
        }
    }

    /// Spends `count` steps; an error value once the budget is gone.
    pub fn spend(&mut self, count: usize) -> Result<(), V> {
        self.steps = self
            .steps
            .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
        if self.steps > MAX_STEPS {
            Err(V::Err(
                "The formula takes too many steps for one record. Simplify it.".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn eval(&mut self, expr: &Expr) -> V {
        if let Err(error) = self.spend(1) {
            return error;
        }
        match expr {
            Expr::Number(number) => V::Num(*number),
            Expr::Text(text) => V::Text(text.clone()),
            Expr::Bool(value) => V::Bool(*value),
            Expr::Field(reference) => self.fields.value(*reference),
            Expr::Neg(inner) => match self.eval(inner).to_num() {
                Ok(number) => V::Num(-number),
                Err(error) => error,
            },
            Expr::Call(name, args) => functions::call(self, name, args),
            Expr::Binary(op, left, right) => self.binary(*op, left, right),
        }
    }

    fn binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> V {
        // and / or stop as soon as the answer is known.
        if matches!(op, BinOp::And | BinOp::Or) {
            let first = match self.eval(left).to_bool() {
                Ok(value) => value,
                Err(error) => return error,
            };
            if (op == BinOp::And && !first) || (op == BinOp::Or && first) {
                return V::Bool(first);
            }
            return match self.eval(right).to_bool() {
                Ok(value) => V::Bool(value),
                Err(error) => error,
            };
        }
        let left = self.eval(left);
        let right = self.eval(right);
        if left.is_err() {
            return left;
        }
        if right.is_err() {
            return right;
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let (a, b) = match (left.to_num(), right.to_num()) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(error), _) | (_, Err(error)) => return error,
                };
                let result = match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    _ if b == 0.0 => return V::Err("Divide by zero".to_owned()),
                    _ => a / b,
                };
                number(result)
            }
            BinOp::Join => match (left.to_text(), right.to_text()) {
                (Ok(a), Ok(b)) => join(a, &b),
                (Err(error), _) | (_, Err(error)) => error,
            },
            _ => match compare(&left, &right) {
                Ok(ordering) => V::Bool(match op {
                    BinOp::Eq => ordering == Ordering::Equal,
                    BinOp::Ne => ordering != Ordering::Equal,
                    BinOp::Lt => ordering == Ordering::Less,
                    BinOp::Le => ordering != Ordering::Greater,
                    BinOp::Gt => ordering == Ordering::Greater,
                    _ => ordering != Ordering::Less,
                }),
                Err(error) => error,
            },
        }
    }
}

/// A number result, or an error for one too large to keep.
pub fn number(value: f64) -> V {
    if value.is_finite() && value.abs() <= crate::fields::MAX_NUMBER {
        V::Num(value)
    } else {
        V::Err("The number is too large.".to_owned())
    }
}

/// How two values order: numbers as numbers, dates as dates, text ignoring
/// case. Blank is 0, "" or false beside the other side.
pub fn compare(left: &V, right: &V) -> Result<Ordering, V> {
    let lower = |a: &V, b: &V| -> Result<Ordering, V> {
        Ok(a.to_text()?
            .to_lowercase()
            .cmp(&b.to_text()?.to_lowercase()))
    };
    Ok(match (left, right) {
        (V::Err(_), _) => return Err(left.clone()),
        (_, V::Err(_)) => return Err(right.clone()),
        (V::Blank, V::Blank) => Ordering::Equal,
        (V::Text(_), V::Text(_)) | (V::Text(_), V::Blank) | (V::Blank, V::Text(_)) => {
            lower(left, right)?
        }
        (V::Time(a), V::Time(b)) => a.cmp(b),
        (V::Date(_) | V::Time(_), _) | (_, V::Date(_) | V::Time(_)) => {
            left.to_date()?.cmp(&right.to_date()?)
        }
        (V::List(_), _) | (_, V::List(_)) => lower(left, right)?,
        (V::Text(text), _) | (_, V::Text(text)) => {
            if text.trim().replace(',', "").parse::<f64>().is_err() {
                return Err(V::Err(format!(
                    "Can't compare \"{}\" with a number.",
                    super::value::short(text)
                )));
            }
            let (a, b) = (left.to_num()?, right.to_num()?);
            a.partial_cmp(&b).unwrap_or(Ordering::Equal)
        }
        _ => {
            let (a, b) = (left.to_num()?, right.to_num()?);
            a.partial_cmp(&b).unwrap_or(Ordering::Equal)
        }
    })
}

/// The type an expression gives, or why it doesn't add up. `field` gives a
/// referenced field's type.
pub fn check(expr: &Expr, field: &dyn Fn(FieldRef) -> Result<Ty, String>) -> Result<Ty, String> {
    match expr {
        Expr::Number(_) => Ok(Ty::Number),
        Expr::Text(_) => Ok(Ty::Text),
        Expr::Bool(_) => Ok(Ty::Boolean),
        Expr::Field(reference) => field(*reference),
        Expr::Neg(inner) => {
            let ty = check(inner, field)?;
            if ty.fits(Ty::Number) {
                Ok(Ty::Number)
            } else {
                Err(format!("- goes before a number, not {}.", ty.name()))
            }
        }
        Expr::Call(name, args) => {
            let types = args
                .iter()
                .map(|arg| check(arg, field))
                .collect::<Result<Vec<_>, _>>()?;
            functions::check(name, &types)
        }
        Expr::Binary(op, left, right) => {
            let a = check(left, field)?;
            let b = check(right, field)?;
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                    for ty in [a, b] {
                        if ty == Ty::Date {
                            return Err(format!(
                                "{} works on numbers. For dates use DATEADD or DATEDIFF.",
                                op_symbol(*op)
                            ));
                        }
                        if !ty.fits(Ty::Number) && ty != Ty::Text {
                            return Err(format!(
                                "{} works on numbers, not {}.",
                                op_symbol(*op),
                                ty.name()
                            ));
                        }
                    }
                    Ok(Ty::Number)
                }
                BinOp::Join => Ok(Ty::Text),
                BinOp::And | BinOp::Or => {
                    for ty in [a, b] {
                        if !(ty.fits(Ty::Boolean) || ty == Ty::Number) {
                            return Err(format!(
                                "and and or join true-or-false parts, not {}.",
                                ty.name()
                            ));
                        }
                    }
                    Ok(Ty::Boolean)
                }
                _ => {
                    let comparable = a.fits(b)
                        || matches!((a, b), (Ty::Date, Ty::Text) | (Ty::Text, Ty::Date))
                        || matches!(
                            (a, b),
                            (Ty::Number, Ty::Boolean) | (Ty::Boolean, Ty::Number)
                        )
                        || matches!((a, b), (Ty::Number, Ty::Text) | (Ty::Text, Ty::Number));
                    if comparable {
                        Ok(Ty::Boolean)
                    } else {
                        Err(format!("Can't compare {} with {}.", a.name(), b.name()))
                    }
                }
            }
        }
    }
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        _ => "/",
    }
}

//! A view's filter (T1.5): a tree of field clauses joined by "and" and
//! "or", at most `MAX_DEPTH` groups deep and `MAX_LEAVES` clauses in all.
//!
//! As stored and written: a list of clauses (`[{field, op, values}, …]`) is
//! the clauses joined by "and", which is what every view stored before
//! trees has, so none needs rewriting; `{"and": [...]}` and `{"or": [...]}`
//! are groups whose items are clauses or groups. A filter that is only
//! clauses joined by "and" is written back as the plain list.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sqlx::{Postgres, QueryBuilder};

use crate::fields::FieldClause;

/// Groups a filter nests at most, counting the outermost.
pub const MAX_DEPTH: usize = 3;
/// Clauses one filter holds at most, across its groups.
pub const MAX_LEAVES: usize = 20;
/// Items one group holds at most.
pub const MAX_GROUP_ITEMS: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Filter {
    Clause(FieldClause),
    /// Every item holds. No items: everything does.
    And(Vec<Filter>),
    /// Any item holds. No items: everything does (an empty group narrows
    /// nothing, as in the editor before a clause is added).
    Or(Vec<Filter>),
}

impl Default for Filter {
    fn default() -> Self {
        Self::And(Vec::new())
    }
}

impl From<Vec<FieldClause>> for Filter {
    fn from(clauses: Vec<FieldClause>) -> Self {
        Self::all(clauses)
    }
}

impl Filter {
    /// The clauses joined by "and".
    pub fn all(clauses: Vec<FieldClause>) -> Self {
        Self::And(clauses.into_iter().map(Self::Clause).collect())
    }

    /// Whether it narrows nothing.
    pub fn is_empty(&self) -> bool {
        self.leaf_count() == 0
    }

    /// Every clause, in order, whatever group it's in.
    pub fn clauses(&self) -> Vec<&FieldClause> {
        let mut found = Vec::new();
        self.collect(&mut found);
        found
    }

    fn collect<'a>(&'a self, found: &mut Vec<&'a FieldClause>) {
        match self {
            Self::Clause(clause) => found.push(clause),
            Self::And(items) | Self::Or(items) => {
                for item in items {
                    item.collect(found);
                }
            }
        }
    }

    /// Every clause, to change in place.
    pub fn clauses_mut(&mut self) -> Vec<&mut FieldClause> {
        match self {
            Self::Clause(clause) => vec![clause],
            Self::And(items) | Self::Or(items) => {
                items.iter_mut().flat_map(Self::clauses_mut).collect()
            }
        }
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            Self::Clause(_) => 1,
            Self::And(items) | Self::Or(items) => items.iter().map(Self::leaf_count).sum(),
        }
    }

    /// Groups deep: a clause alone is 0, a list of clauses 1.
    pub fn depth(&self) -> usize {
        match self {
            Self::Clause(_) => 0,
            Self::And(items) | Self::Or(items) => {
                1 + items.iter().map(Self::depth).max().unwrap_or(0)
            }
        }
    }

    fn widest_group(&self) -> usize {
        match self {
            Self::Clause(_) => 0,
            Self::And(items) | Self::Or(items) => items
                .iter()
                .map(Self::widest_group)
                .max()
                .unwrap_or(0)
                .max(items.len()),
        }
    }

    /// Why the tree is too big, or None.
    pub fn size_problem(&self) -> Option<String> {
        if self.depth() > MAX_DEPTH {
            return Some(format!("A filter nests groups at most {MAX_DEPTH} deep."));
        }
        if self.leaf_count() > MAX_LEAVES {
            return Some(format!("A filter has at most {MAX_LEAVES} conditions."));
        }
        if self.widest_group() > MAX_GROUP_ITEMS {
            return Some(format!("A group holds at most {MAX_GROUP_ITEMS} items."));
        }
        None
    }

    /// The filter with only the clauses `keep` says yes to; groups left
    /// empty go.
    pub fn retain(&self, keep: &dyn Fn(&FieldClause) -> bool) -> Self {
        match self {
            Self::Clause(clause) => {
                if keep(clause) {
                    self.clone()
                } else {
                    Self::And(Vec::new())
                }
            }
            Self::And(items) => Self::And(Self::kept(items, keep)),
            Self::Or(items) => Self::Or(Self::kept(items, keep)),
        }
    }

    fn kept(items: &[Self], keep: &dyn Fn(&FieldClause) -> bool) -> Vec<Self> {
        items
            .iter()
            .map(|item| item.retain(keep))
            .filter(|item| !matches!(item, Self::And(found) | Self::Or(found) if found.is_empty()))
            .collect()
    }

    /// This filter and `more`, both holding.
    pub fn and(self, more: Vec<FieldClause>) -> Self {
        if more.is_empty() {
            return self;
        }
        let mut items = match self {
            Self::And(items) => items,
            other => vec![other],
        };
        items.extend(more.into_iter().map(Self::Clause));
        Self::And(items)
    }

    /// Appends ` AND (...)` for the tree, `leaf` writing each clause as a
    /// condition (`push_leaf` writes one with no leading AND).
    pub fn push_sql(
        &self,
        builder: &mut QueryBuilder<'_, Postgres>,
        push_leaf: &mut dyn FnMut(&mut QueryBuilder<'_, Postgres>, &FieldClause),
    ) {
        if self.is_empty() {
            return;
        }
        builder.push(" AND ");
        self.push_condition(builder, push_leaf);
    }

    fn push_condition(
        &self,
        builder: &mut QueryBuilder<'_, Postgres>,
        push_leaf: &mut dyn FnMut(&mut QueryBuilder<'_, Postgres>, &FieldClause),
    ) {
        match self {
            Self::Clause(clause) => {
                builder.push("(");
                push_leaf(builder, clause);
                builder.push(")");
            }
            Self::And(items) | Self::Or(items) => {
                let items = items
                    .iter()
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    builder.push("true");
                    return;
                }
                let joiner = if matches!(self, Self::And(_)) {
                    " AND "
                } else {
                    " OR "
                };
                builder.push("(");
                for (index, item) in items.into_iter().enumerate() {
                    if index > 0 {
                        builder.push(joiner);
                    }
                    item.push_condition(builder, push_leaf);
                }
                builder.push(")");
            }
        }
    }

    fn to_value(&self, top: bool) -> Value {
        match self {
            Self::Clause(clause) => serde_json::to_value(clause).unwrap_or(Value::Null),
            Self::And(items) if top && items.iter().all(|item| matches!(item, Self::Clause(_))) => {
                Value::Array(items.iter().map(|item| item.to_value(false)).collect())
            }
            Self::And(items) | Self::Or(items) => {
                let key = if matches!(self, Self::And(_)) {
                    "and"
                } else {
                    "or"
                };
                let mut object = Map::new();
                object.insert(
                    key.to_owned(),
                    Value::Array(items.iter().map(|item| item.to_value(false)).collect()),
                );
                Value::Object(object)
            }
        }
    }

    fn from_value(value: Value, depth: usize) -> Result<Self, String> {
        if depth > MAX_DEPTH + 1 {
            return Err(format!("A filter nests groups at most {MAX_DEPTH} deep."));
        }
        let items = |value: Value| -> Result<Vec<Self>, String> {
            match value {
                Value::Array(items) => items
                    .into_iter()
                    .map(|item| Self::from_value(item, depth + 1))
                    .collect(),
                _ => Err("A filter group is a list.".to_owned()),
            }
        };
        match value {
            Value::Array(_) => Ok(Self::And(items(value)?)),
            Value::Object(mut object) if object.len() == 1 && object.contains_key("and") => {
                Ok(Self::And(items(object.remove("and").unwrap_or_default())?))
            }
            Value::Object(mut object) if object.len() == 1 && object.contains_key("or") => {
                Ok(Self::Or(items(object.remove("or").unwrap_or_default())?))
            }
            Value::Object(_) => serde_json::from_value::<FieldClause>(value)
                .map(Self::Clause)
                .map_err(|_| {
                    "A filter is a list of {field, op, values}, or {\"and\": [...]} or {\"or\": [...]}."
                        .to_owned()
                }),
            _ => Err(
                "A filter is a list of {field, op, values}, or {\"and\": [...]} or {\"or\": [...]}."
                    .to_owned(),
            ),
        }
    }
}

impl Serialize for Filter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value(true).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Filter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value, 1).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{Filter, MAX_DEPTH, MAX_LEAVES};
    use crate::fields::{FieldClause, FieldOp};

    fn clause(field: Uuid) -> FieldClause {
        FieldClause {
            field,
            op: FieldOp::NotEmpty,
            values: Vec::new(),
        }
    }

    #[test]
    fn a_flat_list_reads_as_and_and_writes_back_as_a_list() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let stored = json!([
            {"field": a, "op": "notEmpty"},
            {"field": b, "op": "is", "values": ["x"]}
        ]);
        let filter: Filter = serde_json::from_value(stored.clone()).expect("flat");
        assert!(matches!(&filter, Filter::And(items) if items.len() == 2));
        assert_eq!(filter.depth(), 1);
        assert_eq!(serde_json::to_value(&filter).expect("write"), stored);
        let empty: Filter = serde_json::from_value(json!([])).expect("empty");
        assert!(empty.is_empty());
        assert_eq!(serde_json::to_value(&empty).expect("write"), json!([]));
    }

    #[test]
    fn trees_read_and_write_as_groups() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let written = json!({"and": [
            {"field": a, "op": "notEmpty"},
            {"or": [{"field": b, "op": "notEmpty"}, {"field": c, "op": "empty"}]}
        ]});
        let filter: Filter = serde_json::from_value(written.clone()).expect("tree");
        assert_eq!(filter.depth(), 2);
        assert_eq!(filter.leaf_count(), 3);
        assert_eq!(serde_json::to_value(&filter).expect("write"), written);
        assert_eq!(
            filter
                .clauses()
                .iter()
                .map(|clause| clause.field)
                .collect::<Vec<_>>(),
            vec![a, b, c]
        );
        // Without one field, the "or" keeps its other clause.
        let without_b = filter.retain(&|clause| clause.field != b);
        assert_eq!(without_b.leaf_count(), 2);
        let none = filter.retain(&|_| false);
        assert!(none.is_empty());
    }

    #[test]
    fn trees_are_at_most_three_deep_and_twenty_clauses() {
        let field = Uuid::now_v7();
        let leaf = json!({"field": field, "op": "notEmpty"});
        let deep = json!({"and": [{"or": [{"and": [leaf.clone()]}]}]});
        let filter: Filter = serde_json::from_value(deep).expect("three deep");
        assert_eq!(filter.depth(), MAX_DEPTH);
        assert!(filter.size_problem().is_none());
        let too_deep = json!({"and": [{"or": [{"and": [{"or": [leaf.clone()]}]}]}]});
        assert!(
            serde_json::from_value::<Filter>(too_deep.clone())
                .map(|filter| filter.size_problem().is_some())
                .unwrap_or(true),
            "four deep is refused"
        );
        let many = Filter::all((0..=MAX_LEAVES).map(|_| clause(field)).collect());
        assert!(many.size_problem().is_some());
        let enough = Filter::all((0..MAX_LEAVES).map(|_| clause(field)).collect());
        assert!(enough.size_problem().is_none());
        assert!(serde_json::from_value::<Filter>(json!({"nor": []})).is_err());
        assert!(serde_json::from_value::<Filter>(json!("status")).is_err());
    }
}

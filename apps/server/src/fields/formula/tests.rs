//! Formula fields: every function, every limit, renames, and errors as
//! values.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FormulaConfig, RecordData, ResultType, Scope, compile, compute,
    eval::{Fields, Run},
    limits::{MAX_REFERENCES, MAX_SOURCE_CHARACTERS, MAX_STEPS},
    shown,
    value::V,
};
use crate::fields::{Accessor, ChoiceOption, FieldConfig, FieldDefinition, FieldType};

const TAG_A: Uuid = Uuid::from_u128(0xa);
const TAG_B: Uuid = Uuid::from_u128(0xb);
const OPEN: Uuid = Uuid::from_u128(0x0e);
const ADA: Uuid = Uuid::from_u128(0xada);

fn option(id: Uuid, name: &str) -> ChoiceOption {
    ChoiceOption {
        id,
        name: name.to_owned(),
        color: "#336699".to_owned(),
        archived: false,
        category: None,
    }
}

fn field(id: u128, name: &str, field_type: FieldType) -> FieldDefinition {
    let options = match name {
        "Tags" => vec![option(TAG_A, "a"), option(TAG_B, "b")],
        "Stage" => vec![option(OPEN, "Open")],
        _ => Vec::new(),
    };
    FieldDefinition {
        id: Uuid::from_u128(id),
        name: name.to_owned(),
        description: String::new(),
        field_type,
        config: FieldConfig {
            options,
            ..FieldConfig::default()
        },
        position: id as i32,
        archived: false,
        version: 1,
        accessor: Accessor::Stored,
    }
}

fn fields() -> Vec<FieldDefinition> {
    vec![
        field(1, "Price", FieldType::Number),
        field(2, "Qty", FieldType::Number),
        field(3, "Due", FieldType::Date),
        field(4, "Done", FieldType::Checkbox),
        field(5, "Tags", FieldType::MultipleChoice),
        field(6, "Stage", FieldType::SingleChoice),
        field(7, "Note", FieldType::Text),
        field(8, "Owner", FieldType::Person),
        field(9, "Empty", FieldType::Text),
        field(10, "Created", FieldType::CreatedTime),
    ]
}

fn values() -> Value {
    json!({
        Uuid::from_u128(1).to_string(): 12.5,
        Uuid::from_u128(2).to_string(): 4,
        Uuid::from_u128(3).to_string(): "2026-11-20",
        Uuid::from_u128(4).to_string(): true,
        Uuid::from_u128(5).to_string(): [TAG_A, TAG_B],
        Uuid::from_u128(6).to_string(): OPEN,
        Uuid::from_u128(7).to_string(): "  hello   world ",
        Uuid::from_u128(8).to_string(): ADA,
    })
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap()
}

fn formula_field(id: u128, name: &str, config: FormulaConfig) -> FieldDefinition {
    let mut formula = field(id, name, FieldType::Formula);
    formula.accessor = Accessor::Computed;
    formula.config.formula = Some(config);
    formula
}

fn scope(fields: &[FieldDefinition]) -> Scope<'_> {
    Scope {
        fields,
        title_name: "Name",
        own: None,
    }
}

/// Runs every formula of `definitions` on the test record.
fn run_all(definitions: &[FieldDefinition]) -> Value {
    let values = values();
    let labels = HashMap::from([(ADA, "Ada".to_owned())]);
    let record = RecordData {
        title: "Widget",
        number: 7,
        values: &values,
        created_at: Utc.with_ymd_and_hms(2026, 11, 1, 9, 30, 0).unwrap(),
        updated_at: now(),
        created_by: ADA,
        updated_by: ADA,
        labels: &labels,
    };
    compute(definitions, &record, now())
}

/// The value a formula gives on the test record (Null for no value).
fn eval(source: &str) -> Value {
    let mut definitions = fields();
    let config =
        compile(source, &scope(&definitions)).unwrap_or_else(|error| panic!("{source}: {error}"));
    definitions.push(formula_field(100, "F", config));
    run_all(&definitions)
        .get(Uuid::from_u128(100).to_string())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Why a formula is refused when saved.
fn refused(source: &str) -> String {
    match compile(source, &scope(&fields())) {
        Ok(config) => panic!("{source} was accepted as {config:?}"),
        Err(error) => error,
    }
}

fn error_of(value: &Value) -> &str {
    value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{value} isn't an error"))
}

#[test]
fn arithmetic_and_operators() {
    assert_eq!(eval("{Price} * {Qty}"), json!(50));
    assert_eq!(eval("1 + 2 * 3"), json!(7));
    assert_eq!(eval("(1 + 2) * 3"), json!(9));
    assert_eq!(eval("-{Qty} + 1"), json!(-3));
    assert_eq!(eval("10 / 4"), json!(2.5));
    assert_eq!(eval("{Name} & \" (\" & {Qty} & \")\""), json!("Widget (4)"));
    assert_eq!(eval("{Price} > 10"), json!(true));
    assert_eq!(
        eval("{Price} <= 10"),
        Value::Null,
        "false is kept as no value"
    );
    assert_eq!(eval("\"abc\" = \"ABC\""), json!(true));
    assert_eq!(eval("{Qty} <> 4"), Value::Null);
    assert_eq!(eval("{Done} and {Qty} > 3"), json!(true));
    assert_eq!(eval("NOT(ISBLANK({Empty})) or {Qty} > 30"), Value::Null);
    assert_eq!(eval("{Due} > \"2026-11-01\""), json!(true));
}

#[test]
fn logic_functions() {
    assert_eq!(eval("IF({Qty} > 3, \"many\", \"few\")"), json!("many"));
    assert_eq!(eval("IF({Qty} > 30, \"many\")"), Value::Null);
    assert_eq!(
        eval("IFS({Qty} > 10, \"lots\", {Qty} > 3, \"some\")"),
        json!("some")
    );
    assert_eq!(
        eval("SWITCH({Stage}, \"Open\", 1, \"Closed\", 2, 0)"),
        json!(1)
    );
    assert_eq!(eval("SWITCH({Stage}, \"Closed\", 2, 0)"), json!(0));
    assert_eq!(eval("AND({Done}, {Qty} = 4)"), json!(true));
    assert_eq!(eval("OR({Empty}, false)"), Value::Null);
    assert_eq!(eval("NOT({Done})"), Value::Null);
    assert_eq!(eval("ISBLANK({Empty})"), json!(true));
    assert_eq!(eval("COALESCE({Empty}, \"fallback\")"), json!("fallback"));
}

#[test]
fn number_functions() {
    assert_eq!(eval("SUM(1, 2, 3)"), json!(6));
    assert_eq!(eval("SUM({Price}, {Qty})"), json!(16.5));
    assert_eq!(eval("MIN(3, 1, 2)"), json!(1));
    assert_eq!(eval("MAX(3, 1, 2)"), json!(3));
    assert_eq!(eval("AVG(1, 2, 3, 4)"), json!(2.5));
    assert_eq!(eval("AVERAGE(2, 4)"), json!(3));
    assert_eq!(eval("ROUND(2.71828, 2)"), json!(2.72));
    assert_eq!(eval("ROUND(2.5)"), json!(3));
    assert_eq!(eval("FLOOR(2.7)"), json!(2));
    assert_eq!(eval("CEIL(2.1)"), json!(3));
    assert_eq!(eval("ABS(-5)"), json!(5));
    assert_eq!(eval("MOD(7, 3)"), json!(1));
    assert_eq!(eval("POW(2, 10)"), json!(1024));
    assert_eq!(eval("NUMBER(\"1,234.5\")"), json!(1234.5));
}

#[test]
fn text_functions() {
    assert_eq!(eval("CONCAT(\"a\", 1, true)"), json!("a1true"));
    assert_eq!(eval("LEFT(\"hello\", 2)"), json!("he"));
    assert_eq!(eval("RIGHT(\"hello\", 3)"), json!("llo"));
    assert_eq!(eval("LEN(\"héllo\")"), json!(5));
    assert_eq!(eval("LOWER(\"ABC\")"), json!("abc"));
    assert_eq!(eval("UPPER({Name})"), json!("WIDGET"));
    assert_eq!(eval("TRIM({Note})"), json!("hello world"));
    assert_eq!(eval("CONTAINS({Note}, \"WORLD\")"), json!(true));
    assert_eq!(eval("CONTAINS({Tags}, \"a\")"), json!(true));
    assert_eq!(eval("REPLACE(\"a-b-c\", \"-\", \"+\")"), json!("a+b+c"));
    assert_eq!(eval("TEXT(3)"), json!("3"));
    assert_eq!(
        eval("{Owner}"),
        json!("Ada"),
        "a person reads as their name"
    );
    assert_eq!(
        eval("{Stage}"),
        json!("Open"),
        "an option reads as its name"
    );
}

#[test]
fn date_functions() {
    assert_eq!(eval("TODAY()"), json!("2026-11-16"));
    assert_eq!(eval("NOW()"), json!("2026-11-16"));
    assert_eq!(eval("DATEADD({Due}, 1, \"months\")"), json!("2026-12-20"));
    assert_eq!(eval("DATEADD({Due}, 2)"), json!("2026-11-22"));
    assert_eq!(eval("DATEDIFF({Due}, TODAY())"), json!(4));
    assert_eq!(eval("DATEDIFF({Due}, {Created}, \"weeks\")"), json!(2));
    assert_eq!(eval("YEAR({Due})"), json!(2026));
    assert_eq!(eval("MONTH({Due})"), json!(11));
    assert_eq!(eval("DAY({Due})"), json!(20));
    assert_eq!(eval("WEEKDAY({Due})"), json!(5), "2026-11-20 is a Friday");
    assert_eq!(
        eval("FORMATDATE({Due}, \"D MMM YYYY\")"),
        json!("20 Nov 2026")
    );
    assert_eq!(eval("FORMATDATE({Due}, \"dddd\")"), json!("Friday"));
    assert_eq!(eval("FORMATDATE({Created}, \"HH:mm\")"), json!("09:30"));
    assert_eq!(eval("DATE(2026, 13, 1)"), json!("2027-01-01"));
    assert_eq!(eval("DATE(\"2026-02-03\")"), json!("2026-02-03"));
    let error = eval("DATEADD({Due}, 1, \"fortnights\")");
    assert!(error_of(&error).contains("isn't a unit"));
}

#[test]
fn list_functions() {
    assert_eq!(eval("{Tags}"), json!(["a", "b"]));
    assert_eq!(eval("COUNT({Tags})"), json!(2));
    assert_eq!(eval("JOIN({Tags}, \" / \")"), json!("a / b"));
    assert_eq!(eval("FIRST({Tags})"), json!("a"));
    assert_eq!(eval("LAST({Tags})"), json!("b"));
    assert_eq!(eval("UNIQUE({Tags})"), json!(["a", "b"]));
    assert_eq!(eval("COUNT({Empty})"), json!(0));
}

#[test]
fn every_listed_function_runs() {
    // A call for each name, so a function added to the list without a body
    // (or a body without the list) fails here.
    for name in super::FUNCTIONS {
        let source = match name {
            "IF" => "IF(true, 1)".to_owned(),
            "IFS" => "IFS(true, 1)".to_owned(),
            "SWITCH" => "SWITCH(1, 1, 2)".to_owned(),
            "TODAY" | "NOW" => format!("{name}()"),
            "DATE" | "YEAR" | "MONTH" | "DAY" | "WEEKDAY" => format!("{name}(\"2026-01-02\")"),
            "DATEADD" => "DATEADD(\"2026-01-02\", 1)".to_owned(),
            "DATEDIFF" | "FORMATDATE" => format!("{name}(\"2026-01-02\", \"2026-01-01\")"),
            "MOD" | "POW" | "CONTAINS" | "ROUND" | "LEFT" | "RIGHT" | "FLOOR" => {
                format!("{name}(4, 2)")
            }
            "REPLACE" => "REPLACE(\"a\", \"a\", \"b\")".to_owned(),
            "JOIN" | "FIRST" | "LAST" | "UNIQUE" | "COUNT" => format!("{name}({{Tags}})"),
            other => format!("{other}(1)"),
        };
        let value = eval(&source);
        if let Some(error) = value.get("error") {
            assert!(
                !error
                    .as_str()
                    .unwrap_or_default()
                    .contains("isn't a function"),
                "{name} has no body"
            );
        }
    }
}

#[test]
fn errors_are_values() {
    assert_eq!(error_of(&eval("{Price} / 0")), "Divide by zero");
    assert!(error_of(&eval("NUMBER({Note})")).contains("isn't a number"));
    // An error flows through the rest of the formula.
    assert_eq!(error_of(&eval("({Price} / 0) + 1")), "Divide by zero");
    assert_eq!(
        error_of(&eval("IF({Price} / 0 > 1, 1, 2)")),
        "Divide by zero"
    );
    // A result too large to keep.
    assert!(error_of(&eval("POW(10, 400)")).contains("too large"));
}

#[test]
fn mistakes_are_refused_when_saved() {
    assert!(refused("NOPE(1)").contains("no function called NOPE"));
    assert!(refused("{Missing} + 1").contains("no field called \"Missing\""));
    assert!(refused("ABS(1, 2)").contains("ABS takes 1"));
    assert!(refused("{Due} + 1").contains("DATEADD"));
    assert!(refused("{Tags} * 2").contains("not a list"));
    assert!(refused("1 +").contains("ends too soon"));
}

#[test]
fn limits_are_refused_when_saved() {
    let long = format!("\"{}\"", "a".repeat(MAX_SOURCE_CHARACTERS));
    assert!(refused(&long).contains("at most"));
    let many = vec!["{Price}"; MAX_REFERENCES + 1].join(" + ");
    assert!(refused(&many).contains("at most"));
    let deep = format!("{}1{}", "ABS(".repeat(40), ")".repeat(40));
    assert!(refused(&deep).contains("levels deep"));
}

#[test]
fn long_text_is_an_error_value() {
    // 10,000 characters: past what a result keeps.
    let ten_thousand = "REPLACE(REPLACE(REPLACE(REPLACE(\"a\", \"a\", \"aaaaaaaaaa\"), \"a\", \"aaaaaaaaaa\"), \"a\", \"aaaaaaaaaa\"), \"a\", \"aaaaaaaaaa\")";
    assert!(error_of(&eval(ten_thousand)).contains("Text over 5000"));
    // 100,000 characters: past what a formula may build at all.
    let hundred_thousand = format!("REPLACE({ten_thousand}, \"a\", \"aaaaaaaaaa\")");
    assert!(error_of(&eval(&hundred_thousand)).contains("Text over 20000"));
}

struct NoFields;

impl Fields for NoFields {
    fn value(&self, _: super::FieldRef) -> V {
        V::Blank
    }
}

#[test]
fn steps_are_capped() {
    let mut run = Run::new(&NoFields, now());
    assert!(run.spend(MAX_STEPS as usize).is_ok());
    assert!(matches!(run.spend(1), Err(V::Err(message)) if message.contains("too many steps")));
}

#[test]
fn a_formula_cant_use_itself() {
    let mut definitions = fields();
    let a = compile("{Price} * 2", &scope(&definitions)).expect("valid");
    definitions.push(formula_field(100, "A", a));
    let b = compile("{A} + 1", &scope(&definitions)).expect("valid");
    definitions.push(formula_field(101, "B", b));
    let own = Scope {
        fields: &definitions,
        title_name: "Name",
        own: Some(Uuid::from_u128(100)),
    };
    assert!(
        compile("{A} + 1", &own)
            .expect_err("refused")
            .contains("its own field")
    );
    let error = compile("{B} * 2", &own).expect_err("refused");
    assert!(error.contains("use itself"), "{error}");
    assert!(error.contains("\"B\""), "{error}");
}

#[test]
fn formulas_read_formulas_in_order() {
    let mut definitions = fields();
    let a = compile("{Price} * 2", &scope(&definitions)).expect("valid");
    let a_field = formula_field(100, "A", a);
    definitions.push(a_field.clone());
    let b = compile("{A} + 1", &scope(&definitions)).expect("valid");
    // B comes first in the list, but runs after A.
    definitions.pop();
    definitions.push(formula_field(101, "B", b));
    definitions.push(a_field);
    let computed = run_all(&definitions);
    assert_eq!(computed[Uuid::from_u128(100).to_string()], json!(25));
    assert_eq!(computed[Uuid::from_u128(101).to_string()], json!(26));
}

#[test]
fn renaming_a_field_keeps_the_formula_working() {
    let mut definitions = fields();
    let config = compile("{Price} * 2 & {name}", &scope(&definitions)).expect("valid");
    assert_eq!(
        config.source,
        format!("{{#{}}} * 2 & {{#title}}", Uuid::from_u128(1)),
        "stored by ID"
    );
    definitions[0].name = "Cost".to_owned();
    assert_eq!(shown(&config, &definitions, "Name"), "{Cost} * 2 & {Name}");
    definitions.push(formula_field(100, "F", config));
    assert_eq!(
        run_all(&definitions)[Uuid::from_u128(100).to_string()],
        json!("25Widget")
    );
}

#[test]
fn result_types_and_time() {
    let fields = fields();
    let typed = |source: &str| compile(source, &scope(&fields)).expect("valid");
    assert_eq!(typed("{Price} * 2").result, ResultType::Number);
    assert_eq!(typed("{Qty} > 1").result, ResultType::Boolean);
    assert_eq!(typed("DATEADD({Due}, 1)").result, ResultType::Date);
    assert_eq!(typed("{Tags}").result, ResultType::List);
    assert_eq!(typed("IF({Done}, 1, \"no\")").result, ResultType::Text);
    assert!(typed("DATEDIFF({Due}, TODAY())").volatile);
    assert!(!typed("{Price}").volatile);
}

#[test]
fn archived_fields_are_refused_and_read_nothing() {
    let mut definitions = fields();
    definitions[0].archived = true;
    let error = compile("{Price} + 1", &scope(&definitions)).expect_err("refused");
    assert!(error.contains("archived"), "{error}");
    // An archived formula isn't run.
    let mut definitions = fields();
    let config = compile("1 + 1", &scope(&definitions)).expect("valid");
    let mut archived = formula_field(100, "F", config);
    archived.archived = true;
    definitions.push(archived);
    assert_eq!(run_all(&definitions), json!({}));
}

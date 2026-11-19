//! Panel blocks (M1.14): a connected module's panel is a small document of
//! blocks, format `aw-panel/1`, that Workspace draws with its own pieces.
//! This is the server's check of what a module answers, before anything of
//! it reaches a browser: the shape of each block it knows, the limits, and
//! no form that asks for a secret (a module uses a frame for those). A
//! block of a type it doesn't know passes, and the web draws "This panel
//! needs a newer Workspace" in its place. Text is never HTML.
//!
//! `modules/panels/sample.json` holds one of every block; the web's gallery
//! and these tests share it.

use serde_json::{Map, Value};

use super::manifest::Problem;

pub const PANEL_FORMAT: &str = "aw-panel/1";
pub const MAX_BLOCKS: usize = 200;
pub const MAX_PANEL_BYTES: usize = 64 * 1024;
pub const MAX_STATE_BYTES: usize = 8 * 1024;
pub const MAX_TABLE_ROWS: usize = 500;
pub const MAX_FORM_FIELDS: usize = 50;
const MAX_LIST_ITEMS: usize = 500;
const MAX_COLUMNS: usize = 20;
const MAX_BUTTONS: usize = 10;
const MAX_FIELD_ITEMS: usize = 50;
const MAX_CHART_POINTS: usize = 200;
const MAX_SERIES: usize = 10;
const MAX_TEXT_CHARS: usize = 4_000;
const MAX_LABEL_CHARS: usize = 200;
const MAX_RESULT_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_CHARS: usize = 300;

/// The block types this server knows. Any other passes untouched.
pub const BLOCK_TYPES: &[&str] = &[
    "text", "fields", "form", "list", "table", "buttons", "status", "chart", "section",
];
const TEXT_STYLES: &[&str] = &["body", "heading", "muted", "callout"];
const VALUE_KINDS: &[&str] = &[
    "text", "time", "date", "number", "badge", "link", "thing", "person",
];
const BUTTON_STYLES: &[&str] = &["primary", "secondary", "danger"];
const STATES: &[&str] = &["ok", "working", "waiting", "error", "unknown"];
const CHART_KINDS: &[&str] = &["bar", "line", "number"];
const FIELD_TYPES: &[&str] = &["string", "number", "integer", "boolean", "array"];
const WIDGETS: &[&str] = &[
    "text",
    "textarea",
    "select",
    "radio",
    "checkbox",
    "multiselect",
];
const TONES: &[&str] = &[
    "neutral", "accent", "success", "warning", "danger", "agent", "info",
];
/// A form field named like one of these asks for a secret.
const SECRET_WORDS: &[&str] = &["key", "secret", "token", "password"];

struct Check {
    problems: Vec<Problem>,
    blocks: usize,
}

impl Check {
    fn problem(&mut self, path: &str, message: impl Into<String>) {
        if self.problems.len() < 50 {
            self.problems.push(Problem {
                path: path.to_owned(),
                message: message.into(),
            });
        }
    }

    fn text(&mut self, path: &str, value: Option<&Value>, max: usize, required: bool) {
        match value {
            None | Some(Value::Null) if !required => {}
            Some(Value::String(text)) if text.chars().count() <= max => {
                if text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    self.problem(path, "Text has no control characters.");
                }
            }
            Some(Value::String(_)) => self.problem(path, format!("At most {max} characters.")),
            _ => self.problem(path, "Expected text."),
        }
    }

    fn one_of(&mut self, path: &str, value: Option<&Value>, allowed: &[&str]) {
        match value {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if allowed.contains(&text.as_str()) => {}
            _ => self.problem(path, format!("One of: {}.", allowed.join(", "))),
        }
    }

    fn id(&mut self, path: &str, value: Option<&Value>) {
        match value.and_then(Value::as_str) {
            Some(text) if action_id(text) => {}
            _ => self.problem(
                path,
                "An id is 1 to 80 letters, digits, '_', '-', '.' or ':'.",
            ),
        }
    }

    fn action(&mut self, path: &str, value: Option<&Value>, required: bool) {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            if required {
                self.problem(path, "An action is needed here.");
            }
            return;
        };
        let Some(action) = value.as_object() else {
            return self.problem(path, "An action is an object with an id.");
        };
        self.id(&format!("{path}.id"), action.get("id"));
        if let Some(values) = action.get("values")
            && !values.is_object()
        {
            self.problem(&format!("{path}.values"), "Values are an object.");
        }
        self.unknown_keys(path, action, &["id", "values"]);
    }

    fn unknown_keys(&mut self, path: &str, object: &Map<String, Value>, known: &[&str]) {
        if let Some(key) = object.keys().find(|key| !known.contains(&key.as_str())) {
            self.problem(&format!("{path}.{key}"), "Not part of this block.");
        }
    }

    fn array<'a>(
        &mut self,
        path: &str,
        value: Option<&'a Value>,
        max: usize,
        required: bool,
    ) -> &'a [Value] {
        match value {
            None | Some(Value::Null) if !required => &[],
            Some(Value::Array(items)) if items.len() <= max => items,
            Some(Value::Array(_)) => {
                self.problem(path, format!("At most {max} items."));
                &[]
            }
            _ => {
                self.problem(path, "Expected a list.");
                &[]
            }
        }
    }

    fn blocks(&mut self, path: &str, value: Option<&Value>, depth: usize) {
        let blocks = self.array(path, value, MAX_BLOCKS, true);
        for (index, block) in blocks.iter().enumerate() {
            self.block(&format!("{path}[{index}]"), block, depth);
        }
    }

    fn block(&mut self, path: &str, block: &Value, depth: usize) {
        self.blocks += 1;
        if self.blocks == MAX_BLOCKS + 1 {
            self.problem(path, format!("A panel has {MAX_BLOCKS} blocks at most."));
        }
        let Some(object) = block.as_object() else {
            return self.problem(path, "A block is an object with a type.");
        };
        let Some(kind) = object.get("type").and_then(Value::as_str) else {
            return self.problem(&format!("{path}.type"), "A block names its type.");
        };
        if !action_id(kind) {
            return self.problem(&format!("{path}.type"), "A block's type is a short name.");
        }
        let at = |key: &str| format!("{path}.{key}");
        match kind {
            "text" => {
                self.text(&at("text"), object.get("text"), MAX_TEXT_CHARS, true);
                self.one_of(&at("style"), object.get("style"), TEXT_STYLES);
                self.one_of(&at("tone"), object.get("tone"), TONES);
                self.unknown_keys(path, object, &["type", "text", "style", "tone"]);
            }
            "fields" => {
                let items = self.array(&at("items"), object.get("items"), MAX_FIELD_ITEMS, true);
                for (index, item) in items.iter().enumerate() {
                    self.field_item(&format!("{path}.items[{index}]"), item);
                }
                self.unknown_keys(path, object, &["type", "items"]);
            }
            "form" => self.form(path, object),
            "list" => {
                let items = self.array(&at("items"), object.get("items"), MAX_LIST_ITEMS, true);
                for (index, item) in items.iter().enumerate() {
                    let item_path = format!("{path}.items[{index}]");
                    let Some(item) = item.as_object() else {
                        self.problem(&item_path, "A list item is an object.");
                        continue;
                    };
                    self.id(&format!("{item_path}.id"), item.get("id"));
                    self.text(
                        &format!("{item_path}.title"),
                        item.get("title"),
                        MAX_LABEL_CHARS,
                        true,
                    );
                    for key in ["subtitle", "meta"] {
                        self.text(
                            &format!("{item_path}.{key}"),
                            item.get(key),
                            MAX_LABEL_CHARS,
                            false,
                        );
                    }
                    self.badge(&format!("{item_path}.status"), item.get("status"));
                    self.action(&format!("{item_path}.action"), item.get("action"), false);
                    self.unknown_keys(
                        &item_path,
                        item,
                        &["id", "title", "subtitle", "status", "meta", "action"],
                    );
                }
                self.text(&at("empty"), object.get("empty"), MAX_LABEL_CHARS, false);
                if let Some(more) = object.get("more").filter(|more| !more.is_null()) {
                    self.action(&at("more.action"), more.get("action"), true);
                }
                self.unknown_keys(path, object, &["type", "items", "empty", "more"]);
            }
            "table" => self.table(path, object),
            "buttons" => {
                let items = self.array(&at("items"), object.get("items"), MAX_BUTTONS, true);
                for (index, item) in items.iter().enumerate() {
                    let item_path = format!("{path}.items[{index}]");
                    let Some(item) = item.as_object() else {
                        self.problem(&item_path, "A button is an object.");
                        continue;
                    };
                    self.text(&format!("{item_path}.label"), item.get("label"), 60, true);
                    self.action(&format!("{item_path}.action"), item.get("action"), true);
                    self.one_of(
                        &format!("{item_path}.style"),
                        item.get("style"),
                        BUTTON_STYLES,
                    );
                    if let Some(confirm) = item.get("confirm").filter(|value| !value.is_null()) {
                        let confirm_path = format!("{item_path}.confirm");
                        match confirm.as_object() {
                            Some(confirm) => {
                                self.text(
                                    &format!("{confirm_path}.title"),
                                    confirm.get("title"),
                                    MAX_LABEL_CHARS,
                                    true,
                                );
                                self.text(
                                    &format!("{confirm_path}.body"),
                                    confirm.get("body"),
                                    MAX_TEXT_CHARS,
                                    false,
                                );
                                self.unknown_keys(&confirm_path, confirm, &["title", "body"]);
                            }
                            None => self.problem(&confirm_path, "A confirmation has a title."),
                        }
                    }
                    self.unknown_keys(&item_path, item, &["label", "action", "style", "confirm"]);
                }
                self.unknown_keys(path, object, &["type", "items"]);
            }
            "status" => {
                match object.get("state").and_then(Value::as_str) {
                    Some(state) if STATES.contains(&state) => {}
                    _ => self.problem(&at("state"), format!("One of: {}.", STATES.join(", "))),
                }
                self.text(&at("label"), object.get("label"), MAX_LABEL_CHARS, true);
                self.text(&at("detail"), object.get("detail"), MAX_TEXT_CHARS, false);
                if let Some(progress) = object.get("progress").filter(|value| !value.is_null())
                    && !progress
                        .as_f64()
                        .is_some_and(|value| (0.0..=1.0).contains(&value))
                {
                    self.problem(&at("progress"), "Progress is a number from 0 to 1.");
                }
                self.unknown_keys(
                    path,
                    object,
                    &["type", "state", "label", "detail", "progress"],
                );
            }
            "chart" => self.chart(path, object),
            "section" => {
                if depth > 0 {
                    return self.problem(path, "A section can't hold another section.");
                }
                self.text(&at("title"), object.get("title"), MAX_LABEL_CHARS, false);
                self.blocks(&at("blocks"), object.get("blocks"), depth + 1);
                self.unknown_keys(path, object, &["type", "title", "blocks"]);
            }
            // A type this server doesn't know: the web draws a note.
            _ => {}
        }
    }

    fn badge(&mut self, path: &str, value: Option<&Value>) {
        match value {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => {
                self.text(path, Some(&Value::String(text.clone())), 60, true)
            }
            Some(Value::Object(badge)) => {
                self.text(&format!("{path}.text"), badge.get("text"), 60, true);
                self.one_of(&format!("{path}.tone"), badge.get("tone"), TONES);
                self.unknown_keys(path, badge, &["text", "tone"]);
            }
            _ => self.problem(path, "A badge is text, or {text, tone}."),
        }
    }

    fn field_item(&mut self, path: &str, item: &Value) {
        let Some(item) = item.as_object() else {
            return self.problem(path, "A field is an object with a label and value.");
        };
        self.text(&format!("{path}.label"), item.get("label"), 80, true);
        self.one_of(&format!("{path}.kind"), item.get("kind"), VALUE_KINDS);
        let kind = item.get("kind").and_then(Value::as_str).unwrap_or("text");
        self.value(&format!("{path}.value"), item.get("value"), kind);
        self.unknown_keys(path, item, &["label", "value", "kind"]);
    }

    /// A shown value of a kind: plain text, a number, a badge, a link to
    /// https or an app path, or a thing or person by id.
    fn value(&mut self, path: &str, value: Option<&Value>, kind: &str) {
        match (kind, value) {
            (_, None | Some(Value::Null)) => {}
            ("badge", value) => self.badge(path, value),
            ("link", Some(Value::String(href))) => {
                if !safe_href(href) {
                    self.problem(
                        path,
                        "A link goes to an https address or a path in the app.",
                    );
                }
            }
            ("link", Some(Value::Object(link))) => {
                if !link
                    .get("href")
                    .and_then(Value::as_str)
                    .is_some_and(safe_href)
                {
                    self.problem(
                        &format!("{path}.href"),
                        "A link goes to an https address or a path in the app.",
                    );
                }
                self.text(
                    &format!("{path}.text"),
                    link.get("text"),
                    MAX_LABEL_CHARS,
                    false,
                );
                self.unknown_keys(path, link, &["href", "text"]);
            }
            ("thing" | "person", Some(Value::Object(reference))) => {
                if kind == "thing" {
                    self.text(&format!("{path}.kind"), reference.get("kind"), 80, true);
                }
                self.text(&format!("{path}.id"), reference.get("id"), 80, true);
                self.text(
                    &format!("{path}.label"),
                    reference.get("label"),
                    MAX_LABEL_CHARS,
                    false,
                );
                self.unknown_keys(path, reference, &["kind", "id", "label"]);
            }
            ("number", Some(Value::Number(_))) => {}
            (_, Some(Value::String(text))) => self.text(
                path,
                Some(&Value::String(text.clone())),
                MAX_TEXT_CHARS,
                true,
            ),
            (_, Some(Value::Number(_) | Value::Bool(_))) => {}
            _ => self.problem(path, "This value doesn't fit its kind."),
        }
    }

    fn table(&mut self, path: &str, object: &Map<String, Value>) {
        let at = |key: &str| format!("{path}.{key}");
        let columns = self.array(&at("columns"), object.get("columns"), MAX_COLUMNS, true);
        let mut ids = Vec::new();
        for (index, column) in columns.iter().enumerate() {
            let column_path = format!("{path}.columns[{index}]");
            let Some(column) = column.as_object() else {
                self.problem(&column_path, "A column is an object.");
                continue;
            };
            self.id(&format!("{column_path}.id"), column.get("id"));
            self.text(
                &format!("{column_path}.label"),
                column.get("label"),
                80,
                true,
            );
            self.one_of(
                &format!("{column_path}.kind"),
                column.get("kind"),
                VALUE_KINDS,
            );
            self.unknown_keys(&column_path, column, &["id", "label", "kind"]);
            if let Some(id) = column.get("id").and_then(Value::as_str) {
                ids.push((
                    id.to_owned(),
                    column
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("text")
                        .to_owned(),
                ));
            }
        }
        let rows = self.array(&at("rows"), object.get("rows"), MAX_TABLE_ROWS, true);
        for (index, row) in rows.iter().enumerate() {
            let row_path = format!("{path}.rows[{index}]");
            let Some(row) = row.as_object() else {
                self.problem(&row_path, "A row is an object with an id and cells.");
                continue;
            };
            self.id(&format!("{row_path}.id"), row.get("id"));
            match row.get("cells").and_then(Value::as_object) {
                Some(cells) => {
                    for (key, cell) in cells {
                        match ids.iter().find(|(id, _)| id == key) {
                            Some((_, kind)) => {
                                self.value(&format!("{row_path}.cells.{key}"), Some(cell), kind)
                            }
                            None => self.problem(
                                &format!("{row_path}.cells.{key}"),
                                "A cell names one of the table's columns.",
                            ),
                        }
                    }
                }
                None => self.problem(&format!("{row_path}.cells"), "Cells are an object."),
            }
            self.unknown_keys(&row_path, row, &["id", "cells"]);
        }
        self.action(&at("rowAction"), object.get("rowAction"), false);
        self.unknown_keys(path, object, &["type", "columns", "rows", "rowAction"]);
    }

    fn chart(&mut self, path: &str, object: &Map<String, Value>) {
        let at = |key: &str| format!("{path}.{key}");
        let kind = object.get("kind").and_then(Value::as_str);
        if !kind.is_some_and(|kind| CHART_KINDS.contains(&kind)) {
            self.problem(&at("kind"), format!("One of: {}.", CHART_KINDS.join(", ")));
        }
        self.text(&at("unit"), object.get("unit"), 20, false);
        if kind == Some("number") {
            if !object.get("value").is_some_and(Value::is_number) {
                self.problem(&at("value"), "A number chart has a value.");
            }
            self.unknown_keys(path, object, &["type", "kind", "value", "unit"]);
            return;
        }
        let x = self.array(&at("x"), object.get("x"), MAX_CHART_POINTS, true);
        for (index, label) in x.iter().enumerate() {
            self.text(&format!("{path}.x[{index}]"), Some(label), 40, true);
        }
        let series = self.array(&at("series"), object.get("series"), MAX_SERIES, true);
        for (index, one) in series.iter().enumerate() {
            let series_path = format!("{path}.series[{index}]");
            let Some(one) = one.as_object() else {
                self.problem(
                    &series_path,
                    "A series is an object with a name and values.",
                );
                continue;
            };
            self.text(&format!("{series_path}.name"), one.get("name"), 60, true);
            match one.get("values").and_then(Value::as_array) {
                Some(values)
                    if values.len() == x.len()
                        && values
                            .iter()
                            .all(|value| value.is_number() || value.is_null()) => {}
                _ => self.problem(
                    &format!("{series_path}.values"),
                    "A series has one number for each x.",
                ),
            }
            self.unknown_keys(&series_path, one, &["name", "values"]);
        }
        self.unknown_keys(path, object, &["type", "kind", "x", "series", "unit"]);
    }

    fn form(&mut self, path: &str, object: &Map<String, Value>) {
        let at = |key: &str| format!("{path}.{key}");
        self.id(&at("id"), object.get("id"));
        let schema = object.get("schema").and_then(Value::as_object);
        let properties = schema.and_then(|schema| schema.get("properties"));
        match (schema, properties.and_then(Value::as_object)) {
            (Some(schema), Some(properties))
                if schema.get("type").and_then(Value::as_str) == Some("object") =>
            {
                if properties.len() > MAX_FORM_FIELDS {
                    self.problem(
                        &at("schema.properties"),
                        format!("A form has {MAX_FORM_FIELDS} fields at most."),
                    );
                }
                for (name, field) in properties {
                    self.form_field(&format!("{path}.schema.properties.{name}"), name, field);
                }
                if let Some(required) = schema.get("required")
                    && !required.as_array().is_some_and(|names| {
                        names.iter().all(|name| {
                            name.as_str()
                                .is_some_and(|name| properties.contains_key(name))
                        })
                    })
                {
                    self.problem(&at("schema.required"), "Required names the form's fields.");
                }
                self.text(
                    &at("schema.title"),
                    schema.get("title"),
                    MAX_LABEL_CHARS,
                    false,
                );
                self.text(
                    &at("schema.description"),
                    schema.get("description"),
                    MAX_TEXT_CHARS,
                    false,
                );
            }
            _ => self.problem(
                &at("schema"),
                "A form's schema is an object schema with properties.",
            ),
        }
        if let Some(ui) = object.get("ui").filter(|ui| !ui.is_null()) {
            match ui.as_object() {
                Some(ui) => {
                    if let Some(order) = ui.get("order")
                        && !order
                            .as_array()
                            .is_some_and(|order| order.iter().all(Value::is_string))
                    {
                        self.problem(&at("ui.order"), "The order is a list of field names.");
                    }
                    if let Some(widgets) = ui.get("widgets") {
                        match widgets.as_object() {
                            Some(widgets) => {
                                for (name, widget) in widgets {
                                    self.one_of(
                                        &format!("{path}.ui.widgets.{name}"),
                                        Some(widget),
                                        WIDGETS,
                                    );
                                }
                            }
                            None => self.problem(&at("ui.widgets"), "Widgets are an object."),
                        }
                    }
                    self.unknown_keys(&at("ui"), ui, &["order", "widgets"]);
                }
                None => self.problem(&at("ui"), "The form's ui is an object."),
            }
        }
        if let Some(values) = object.get("values").filter(|values| !values.is_null())
            && !values.is_object()
        {
            self.problem(&at("values"), "Values are an object.");
        }
        match object.get("submit").and_then(Value::as_object) {
            Some(submit) => {
                self.text(&at("submit.label"), submit.get("label"), 60, true);
                self.action(&at("submit.action"), submit.get("action"), true);
                self.unknown_keys(&at("submit"), submit, &["label", "action"]);
            }
            None => self.problem(&at("submit"), "A form has a submit button."),
        }
        if let Some(cancel) = object.get("cancel").filter(|cancel| !cancel.is_null()) {
            match cancel.as_object() {
                Some(cancel) => {
                    self.text(&at("cancel.label"), cancel.get("label"), 60, true);
                    self.action(&at("cancel.action"), cancel.get("action"), false);
                    self.unknown_keys(&at("cancel"), cancel, &["label", "action"]);
                }
                None => self.problem(&at("cancel"), "Cancel is a button with a label."),
            }
        }
        self.unknown_keys(
            path,
            object,
            &["type", "id", "schema", "ui", "values", "submit", "cancel"],
        );
    }

    fn form_field(&mut self, path: &str, name: &str, field: &Value) {
        if !action_id(name) {
            self.problem(path, "A field's name is a short id.");
        }
        let lower = name.to_ascii_lowercase();
        if SECRET_WORDS.iter().any(|word| lower.contains(word)) {
            self.problem(
                path,
                "A panel form can't ask for a key, secret, token or password; use a frame.",
            );
        }
        let Some(field) = field.as_object() else {
            return self.problem(path, "A field is a schema object.");
        };
        if field.get("format").and_then(Value::as_str) == Some("password")
            || field.get("writeOnly").and_then(Value::as_bool) == Some(true)
        {
            self.problem(
                path,
                "A panel form can't ask for a key, secret, token or password; use a frame.",
            );
        }
        let kind = field.get("type").and_then(Value::as_str);
        if !kind.is_some_and(|kind| FIELD_TYPES.contains(&kind)) && !field.contains_key("enum") {
            self.problem(
                &format!("{path}.type"),
                "A field is a string, number, integer, boolean, enum or list of an enum.",
            );
        }
        if kind == Some("array") {
            let enumerated = field
                .get("items")
                .and_then(|items| items.get("enum"))
                .is_some_and(Value::is_array);
            if !enumerated {
                self.problem(&format!("{path}.items"), "A list field picks from an enum.");
            }
        }
        if let Some(choices) = field.get("enum")
            && !choices.as_array().is_some_and(|choices| {
                choices.len() <= 100
                    && choices
                        .iter()
                        .all(|choice| choice.is_string() || choice.is_number())
            })
        {
            self.problem(&format!("{path}.enum"), "An enum lists up to 100 choices.");
        }
        self.text(&format!("{path}.title"), field.get("title"), 80, false);
        self.text(
            &format!("{path}.description"),
            field.get("description"),
            MAX_LABEL_CHARS * 2,
            false,
        );
        if let Some(pattern) = field.get("pattern").and_then(Value::as_str)
            && pattern.len() > 200
        {
            self.problem(
                &format!("{path}.pattern"),
                "A pattern is 200 characters at most.",
            );
        }
        self.unknown_keys(
            path,
            field,
            &[
                "type",
                "title",
                "description",
                "default",
                "minimum",
                "maximum",
                "maxLength",
                "minLength",
                "pattern",
                "enum",
                "items",
                "format",
            ],
        );
    }
}

/// An action, row or field id.
fn action_id(text: &str) -> bool {
    (1..=80).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

/// An https address, or a path in this app (one leading slash).
pub fn safe_href(href: &str) -> bool {
    if href.len() > 2000 || href.chars().any(char::is_control) {
        return false;
    }
    if let Some(rest) = href.strip_prefix('/') {
        return !rest.starts_with('/') && !rest.starts_with('\\');
    }
    url::Url::parse(href).is_ok_and(|url| url.scheme() == "https" && url.host().is_some())
}

/// Checks a panel document as a module answered it.
pub fn check(document: &Value) -> Result<(), Vec<Problem>> {
    let mut check = Check {
        problems: Vec::new(),
        blocks: 0,
    };
    let Some(object) = document.as_object() else {
        check.problem("panel", "A panel is an object.");
        return Err(check.problems);
    };
    if serde_json::to_vec(document).map_or(true, |bytes| bytes.len() > MAX_PANEL_BYTES) {
        check.problem("panel", "A panel is 64 KB at most.");
        return Err(check.problems);
    }
    if object.get("format").and_then(Value::as_str) != Some(PANEL_FORMAT) {
        check.problem("format", format!("A panel's format is \"{PANEL_FORMAT}\"."));
    }
    check.text("title", object.get("title"), 120, false);
    check.blocks("blocks", object.get("blocks"), 0);
    match object.get("state") {
        None | Some(Value::Null) => {}
        Some(Value::String(state)) if state.len() <= MAX_STATE_BYTES => {}
        _ => check.problem("state", "State is a string of 8 KB at most."),
    }
    if let Some(refresh) = object.get("refresh").filter(|value| !value.is_null()) {
        let seconds = refresh.get("afterSeconds").and_then(Value::as_u64);
        if !seconds.is_some_and(|seconds| (5..=3600).contains(&seconds)) {
            check.problem(
                "refresh.afterSeconds",
                "A panel refreshes after 5 to 3600 seconds.",
            );
        }
    }
    check.unknown_keys(
        "panel",
        object,
        &["format", "title", "blocks", "state", "refresh"],
    );
    if check.problems.is_empty() {
        Ok(())
    } else {
        Err(check.problems)
    }
}

/// What a module answered a panel request or action with.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Draw this.
    Panel(Value),
    /// Show these under the form's fields.
    Errors(Map<String, Value>),
    /// The flow is finished.
    Done {
        message: Option<String>,
        result: Option<Value>,
    },
    Close,
}

/// Reads a module's answer, or says what's wrong with it.
pub fn read_answer(answer: &Value) -> Result<Answer, Vec<Problem>> {
    let one = |path: &str, message: &str| {
        Err(vec![Problem {
            path: path.to_owned(),
            message: message.to_owned(),
        }])
    };
    let Some(object) = answer.as_object() else {
        return one("answer", "An answer is an object.");
    };
    if object.len() != 1 {
        return one(
            "answer",
            "An answer is one of panel, errors, done or close.",
        );
    }
    let (key, value) = object.iter().next().expect("one entry");
    match key.as_str() {
        "panel" => {
            check(value)?;
            Ok(Answer::Panel(value.clone()))
        }
        "errors" => {
            let Some(errors) = value.as_object() else {
                return one("errors", "Errors are messages by field.");
            };
            let fine = errors.len() <= MAX_FORM_FIELDS
                && errors.iter().all(|(field, message)| {
                    action_id(field)
                        && message.as_str().is_some_and(|text| {
                            (1..=MAX_MESSAGE_CHARS).contains(&text.chars().count())
                        })
                });
            if !fine {
                return one("errors", "Errors are short messages by field.");
            }
            Ok(Answer::Errors(errors.clone()))
        }
        "done" => {
            let Some(done) = value.as_object() else {
                return one("done", "Done is an object.");
            };
            let message = match done.get("message") {
                None | Some(Value::Null) => None,
                Some(Value::String(text))
                    if text.chars().count() <= MAX_MESSAGE_CHARS
                        && !text.chars().any(char::is_control) =>
                {
                    Some(text.clone())
                }
                _ => return one("done.message", "A short line of text."),
            };
            let result = done.get("result").filter(|value| !value.is_null()).cloned();
            if result
                .as_ref()
                .is_some_and(|result| result.to_string().len() > MAX_RESULT_BYTES)
            {
                return one("done.result", "A result is 16 KB at most.");
            }
            Ok(Answer::Done { message, result })
        }
        "close" if value.as_bool() == Some(true) => Ok(Answer::Close),
        "frame" => one("frame", "This Workspace doesn't open framed panels yet."),
        _ => one(
            "answer",
            "An answer is one of panel, errors, done or close.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample() -> Value {
        serde_json::from_str(include_str!("../../../../../modules/panels/sample.json"))
            .expect("the shared sample panel")
    }

    fn paths(document: &Value) -> Vec<String> {
        check(document)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|problem| problem.path)
            .collect()
    }

    #[test]
    fn the_shared_sample_has_every_block_and_passes() {
        let sample = sample();
        assert_eq!(check(&sample), Ok(()));
        let mut seen: Vec<&str> = Vec::new();
        fn walk<'a>(blocks: &'a Value, seen: &mut Vec<&'a str>) {
            for block in blocks.as_array().into_iter().flatten() {
                if let Some(kind) = block.get("type").and_then(Value::as_str) {
                    seen.push(kind);
                }
                if let Some(inner) = block.get("blocks") {
                    walk(inner, seen);
                }
            }
        }
        walk(&sample["blocks"], &mut seen);
        for kind in BLOCK_TYPES {
            assert!(seen.contains(kind), "the sample lacks a {kind} block");
        }
    }

    #[test]
    fn each_block_is_checked_for_its_shape() {
        let cases = [
            (json!({"type": "text"}), "blocks[0].text"),
            (
                json!({"type": "text", "text": "x", "style": "shout"}),
                "blocks[0].style",
            ),
            (
                json!({"type": "fields", "items": [{"label": "Link", "value": "javascript:alert(1)", "kind": "link"}]}),
                "blocks[0].items[0].value",
            ),
            (
                json!({"type": "list", "items": [{"id": "a"}]}),
                "blocks[0].items[0].title",
            ),
            (
                json!({"type": "table", "columns": [{"id": "a", "label": "A"}], "rows": [{"id": "1", "cells": {"b": "x"}}]}),
                "blocks[0].rows[0].cells.b",
            ),
            (
                json!({"type": "buttons", "items": [{"label": "Go"}]}),
                "blocks[0].items[0].action",
            ),
            (
                json!({"type": "status", "state": "fine", "label": "x"}),
                "blocks[0].state",
            ),
            (
                json!({"type": "status", "state": "ok", "label": "x", "progress": 2}),
                "blocks[0].progress",
            ),
            (
                json!({"type": "chart", "kind": "bar", "x": ["a", "b"], "series": [{"name": "s", "values": [1]}]}),
                "blocks[0].series[0].values",
            ),
            (
                json!({"type": "section", "blocks": [{"type": "section", "blocks": []}]}),
                "blocks[0].blocks[0]",
            ),
            (
                json!({"type": "text", "text": "x", "html": "<b>x</b>"}),
                "blocks[0].html",
            ),
        ];
        for (block, path) in cases {
            let document = json!({"format": PANEL_FORMAT, "blocks": [block]});
            let found = paths(&document);
            assert!(found.contains(&path.to_owned()), "{path}: {found:?}");
        }
    }

    #[test]
    fn limits_are_held() {
        let many = vec![json!({"type": "text", "text": "x"}); MAX_BLOCKS + 1];
        assert!(
            paths(&json!({"format": PANEL_FORMAT, "blocks": many})).contains(&"blocks".to_owned())
        );
        // Sections count their blocks too.
        let section =
            json!({"type": "section", "blocks": vec![json!({"type": "text", "text": "x"}); 150]});
        let nested = json!({"format": PANEL_FORMAT, "blocks": [section.clone(), section]});
        assert!(check(&nested).is_err());
        let rows: Vec<Value> = (0..=MAX_TABLE_ROWS)
            .map(|row| json!({"id": row.to_string(), "cells": {}}))
            .collect();
        let table = json!({"format": PANEL_FORMAT, "blocks": [{"type": "table", "columns": [], "rows": rows}]});
        assert!(paths(&table).contains(&"blocks[0].rows".to_owned()));
        let long = vec![json!({"type": "text", "text": "x".repeat(4000)}); 20];
        let big = json!({"format": PANEL_FORMAT, "blocks": long});
        assert_eq!(paths(&big), vec!["panel".to_owned()]);
        let fields: Map<String, Value> = (0..=MAX_FORM_FIELDS)
            .map(|index| (format!("f{index}"), json!({"type": "string"})))
            .collect();
        let form = json!({"format": PANEL_FORMAT, "blocks": [{"type": "form", "id": "f",
            "schema": {"type": "object", "properties": fields},
            "submit": {"label": "Save", "action": {"id": "save"}}}]});
        assert!(paths(&form).contains(&"blocks[0].schema.properties".to_owned()));
        assert!(paths(&json!({"format": PANEL_FORMAT, "blocks": [], "state": "x".repeat(MAX_STATE_BYTES + 1)})).contains(&"state".to_owned()));
    }

    #[test]
    fn a_form_never_asks_for_a_secret() {
        for field in [
            json!({"apiKey": {"type": "string"}}),
            json!({"login": {"type": "string", "format": "password"}}),
            json!({"login": {"type": "string", "writeOnly": true}}),
            json!({"access_token": {"type": "string"}}),
        ] {
            let name = field
                .as_object()
                .and_then(|field| field.keys().next().cloned())
                .expect("name");
            let form = json!({"format": PANEL_FORMAT, "blocks": [{"type": "form", "id": "f",
                "schema": {"type": "object", "properties": field},
                "submit": {"label": "Save", "action": {"id": "save"}}}]});
            assert!(
                paths(&form).contains(&format!("blocks[0].schema.properties.{name}")),
                "{name}"
            );
        }
    }

    #[test]
    fn an_unknown_block_passes_for_the_web_to_note() {
        let document = json!({"format": PANEL_FORMAT, "blocks": [{"type": "map", "lat": 1}]});
        assert_eq!(check(&document), Ok(()));
        let bad = json!({"format": PANEL_FORMAT, "blocks": [{"type": "<script>"}]});
        assert!(paths(&bad).contains(&"blocks[0].type".to_owned()));
        assert!(
            paths(&json!({"format": "aw-panel/2", "blocks": []})).contains(&"format".to_owned())
        );
    }

    #[test]
    fn answers_are_one_of_four() {
        assert!(matches!(
            read_answer(&json!({"panel": sample()})),
            Ok(Answer::Panel(_))
        ));
        assert!(matches!(
            read_answer(&json!({"errors": {"build": "No such build."}})),
            Ok(Answer::Errors(_))
        ));
        assert_eq!(
            read_answer(
                &json!({"done": {"message": "Rerun started.", "result": {"build": "215"}}})
            ),
            Ok(Answer::Done {
                message: Some("Rerun started.".into()),
                result: Some(json!({"build": "215"})),
            })
        );
        assert_eq!(read_answer(&json!({"close": true})), Ok(Answer::Close));
        assert!(read_answer(&json!({"frame": {"url": "https://x"}})).is_err());
        assert!(read_answer(&json!({"panel": {}, "close": true})).is_err());
        assert!(read_answer(&json!({"errors": {"build": 3}})).is_err());
    }

    #[test]
    fn links_go_to_https_or_the_app() {
        assert!(safe_href("https://builds.example.com/214"));
        assert!(safe_href("/w/acme/s/game"));
        for bad in [
            "http://x.test",
            "javascript:alert(1)",
            "//evil.test",
            "/\\evil",
            "data:text/html,x",
        ] {
            assert!(!safe_href(bad), "{bad}");
        }
    }
}

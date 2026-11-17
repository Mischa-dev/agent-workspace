//! Reading a formula's source into an expression, and writing one back.
//!
//! ```text
//! expr     := or
//! or       := and ("or" and)*
//! and      := compare ("and" compare)*
//! compare  := join (("=" | "!=" | "<>" | "<" | "<=" | ">" | ">=") join)?
//! join     := sum ("&" sum)*
//! sum      := product (("+" | "-") product)*
//! product  := unary (("*" | "/") unary)*
//! unary    := "-" unary | primary
//! primary  := number | "text" | true | false | "{" name "}" | NAME "(" args ")" | "(" expr ")"
//! ```
//!
//! A field is written by name, `{Due date}`, and stored by ID,
//! `{#<field id>}`; the record's title is `{#title}`. Text is in double
//! quotes, with `""` for a quote inside it.

use uuid::Uuid;

use super::limits::MAX_DEPTH;

/// What a field reference names.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldRef {
    /// The record's title.
    Title,
    Field(Uuid),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Join,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Join => "&",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// Binding strength: higher binds tighter.
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => 3,
            Self::Join => 4,
            Self::Add | Self::Sub => 5,
            Self::Mul | Self::Div => 6,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Field(FieldRef),
    /// A function call; the name is upper case.
    Call(String, Vec<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

impl Expr {
    /// Every field reference, in order, each mention once.
    pub fn references(&self) -> Vec<FieldRef> {
        let mut found = Vec::new();
        self.collect(&mut found);
        found
    }

    fn collect(&self, found: &mut Vec<FieldRef>) {
        match self {
            Self::Field(reference) => found.push(*reference),
            Self::Call(_, args) => args.iter().for_each(|arg| arg.collect(found)),
            Self::Binary(_, left, right) => {
                left.collect(found);
                right.collect(found);
            }
            Self::Neg(inner) => inner.collect(found),
            Self::Number(_) | Self::Text(_) | Self::Bool(_) => {}
        }
    }

    /// The names of the functions it calls.
    pub fn calls(&self) -> Vec<&str> {
        let mut found = Vec::new();
        self.collect_calls(&mut found);
        found
    }

    fn collect_calls<'a>(&'a self, found: &mut Vec<&'a str>) {
        match self {
            Self::Call(name, args) => {
                found.push(name);
                args.iter().for_each(|arg| arg.collect_calls(found));
            }
            Self::Binary(_, left, right) => {
                left.collect_calls(found);
                right.collect_calls(found);
            }
            Self::Neg(inner) => inner.collect_calls(found),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Field(String),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
}

fn tokens(source: &str) -> Result<Vec<Token>, String> {
    let characters = source.chars().collect::<Vec<_>>();
    let mut found = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        if character.is_ascii_digit()
            || (character == '.' && characters.get(index + 1).is_some_and(char::is_ascii_digit))
        {
            let start = index;
            while index < characters.len()
                && (characters[index].is_ascii_digit() || characters[index] == '.')
            {
                index += 1;
            }
            if index < characters.len() && matches!(characters[index], 'e' | 'E') {
                let mut look = index + 1;
                if look < characters.len() && matches!(characters[look], '+' | '-') {
                    look += 1;
                }
                if look < characters.len() && characters[look].is_ascii_digit() {
                    index = look;
                    while index < characters.len() && characters[index].is_ascii_digit() {
                        index += 1;
                    }
                }
            }
            let written = characters[start..index].iter().collect::<String>();
            let number = written
                .parse::<f64>()
                .map_err(|_| format!("{written} isn't a number."))?;
            found.push(Token::Number(number));
            continue;
        }
        if character == '"' {
            let mut text = String::new();
            index += 1;
            loop {
                match characters.get(index) {
                    None => return Err("A text in quotes isn't closed: add a \".".to_owned()),
                    Some('"') if characters.get(index + 1) == Some(&'"') => {
                        text.push('"');
                        index += 2;
                    }
                    Some('"') => {
                        index += 1;
                        break;
                    }
                    Some(other) => {
                        text.push(*other);
                        index += 1;
                    }
                }
            }
            found.push(Token::Text(text));
            continue;
        }
        if character == '{' {
            let mut name = String::new();
            index += 1;
            loop {
                match characters.get(index) {
                    None => {
                        return Err("A field name in { } isn't closed: add a }.".to_owned());
                    }
                    Some('}') => {
                        index += 1;
                        break;
                    }
                    Some(other) => {
                        name.push(*other);
                        index += 1;
                    }
                }
            }
            found.push(Token::Field(name.trim().to_owned()));
            continue;
        }
        if character.is_alphabetic() || character == '_' {
            let start = index;
            while index < characters.len()
                && (characters[index].is_alphanumeric() || characters[index] == '_')
            {
                index += 1;
            }
            found.push(Token::Name(characters[start..index].iter().collect()));
            continue;
        }
        let next = characters.get(index + 1).copied();
        let (op, width) = match (character, next) {
            ('<', Some('=')) => ("<=", 2),
            ('>', Some('=')) => (">=", 2),
            ('!', Some('=')) => ("!=", 2),
            ('<', Some('>')) => ("!=", 2),
            ('=', Some('=')) => ("=", 2),
            ('&', Some('&')) => ("and", 2),
            ('|', Some('|')) => ("or", 2),
            ('+', _) => ("+", 1),
            ('-', _) => ("-", 1),
            ('*', _) => ("*", 1),
            ('/', _) => ("/", 1),
            ('&', _) => ("&", 1),
            ('=', _) => ("=", 1),
            ('<', _) => ("<", 1),
            ('>', _) => (">", 1),
            ('(', _) => {
                found.push(Token::Open);
                index += 1;
                continue;
            }
            (')', _) => {
                found.push(Token::Close);
                index += 1;
                continue;
            }
            (',' | ';', _) => {
                found.push(Token::Comma);
                index += 1;
                continue;
            }
            _ => return Err(format!("\"{character}\" can't be used here.")),
        };
        found.push(Token::Op(op));
        index += width;
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// Parsing

/// Parses a formula; `field` turns what's inside `{ }` into a reference,
/// or says why it can't.
pub fn parse(
    source: &str,
    field: &dyn Fn(&str) -> Result<FieldRef, String>,
) -> Result<Expr, String> {
    let tokens = tokens(source)?;
    if tokens.is_empty() {
        return Err("Write a formula, such as {Price} * {Quantity}.".to_owned());
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
        field,
    };
    let expr = parser.expr()?;
    if parser.at < parser.tokens.len() {
        return Err(format!(
            "Something follows the end of the formula: {}.",
            describe(&parser.tokens[parser.at])
        ));
    }
    Ok(expr)
}

fn describe(token: &Token) -> String {
    match token {
        Token::Number(number) => format!("{number}"),
        Token::Text(text) => format!("\"{text}\""),
        Token::Field(name) => format!("{{{name}}}"),
        Token::Name(name) => name.clone(),
        Token::Op(op) => (*op).to_owned(),
        Token::Open => "(".to_owned(),
        Token::Close => ")".to_owned(),
        Token::Comma => ",".to_owned(),
    }
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
    field: &'a dyn Fn(&str) -> Result<FieldRef, String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!(
                "The formula nests more than {MAX_DEPTH} levels deep. Split it into simpler parts."
            ));
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.deeper()?;
        let expr = self.binary(1);
        self.depth -= 1;
        expr
    }

    /// The operator at the cursor, if it binds at least as tightly as `min`.
    fn operator(&self, min: u8) -> Option<BinOp> {
        let op = match self.peek()? {
            Token::Op(op) => match *op {
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "&" => BinOp::Join,
                "=" => BinOp::Eq,
                "!=" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                "and" => BinOp::And,
                "or" => BinOp::Or,
                _ => return None,
            },
            Token::Name(name) if name.eq_ignore_ascii_case("and") => BinOp::And,
            Token::Name(name) if name.eq_ignore_ascii_case("or") => BinOp::Or,
            _ => return None,
        };
        (op.precedence() >= min).then_some(op)
    }

    fn binary(&mut self, min: u8) -> Result<Expr, String> {
        let mut left = self.unary()?;
        let mut levels = 0;
        while let Some(op) = self.operator(min) {
            self.at += 1;
            let right = self.binary(op.precedence() + 1)?;
            // A comparison takes one on each side: a = b = c is refused.
            if op.precedence() == 3
                && matches!(&left, Expr::Binary(inner, _, _) if inner.precedence() == 3)
            {
                return Err(
                    "Compare two things at a time; join comparisons with and or or.".to_owned(),
                );
            }
            left = Expr::Binary(op, Box::new(left), Box::new(right));
            levels += 1;
            if levels > MAX_DEPTH * 4 {
                return Err("The formula is too long. Split it into simpler parts.".to_owned());
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::Op("-")) {
            self.at += 1;
            self.deeper()?;
            let inner = self.unary();
            self.depth -= 1;
            return Ok(match inner? {
                Expr::Number(number) => Expr::Number(-number),
                other => Expr::Neg(Box::new(other)),
            });
        }
        if self.peek() == Some(&Token::Op("+")) {
            self.at += 1;
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let Some(token) = self.tokens.get(self.at).cloned() else {
            return Err("The formula ends too soon: something is missing at the end.".to_owned());
        };
        self.at += 1;
        match token {
            Token::Number(number) => Ok(Expr::Number(number)),
            Token::Text(text) => Ok(Expr::Text(text)),
            Token::Field(name) => (self.field)(&name).map(Expr::Field),
            Token::Open => {
                let inner = self.expr()?;
                if self.peek() != Some(&Token::Close) {
                    return Err("A ( isn't closed: add a ).".to_owned());
                }
                self.at += 1;
                Ok(inner)
            }
            Token::Name(name) => {
                if self.peek() == Some(&Token::Open) {
                    self.at += 1;
                    self.deeper()?;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Token::Close) {
                        self.at += 1;
                    } else {
                        loop {
                            args.push(self.expr()?);
                            match self.peek() {
                                Some(Token::Comma) => self.at += 1,
                                Some(Token::Close) => {
                                    self.at += 1;
                                    break;
                                }
                                _ => {
                                    return Err(format!(
                                        "{}( needs a , between its parts and a ) at its end.",
                                        name.to_uppercase()
                                    ));
                                }
                            }
                        }
                    }
                    self.depth -= 1;
                    return Ok(Expr::Call(name.to_uppercase(), args));
                }
                match name.to_ascii_lowercase().as_str() {
                    "true" => Ok(Expr::Bool(true)),
                    "false" => Ok(Expr::Bool(false)),
                    _ => Err(format!(
                        "\"{name}\" isn't something a formula knows. Put a field's name in {{ }}, such as {{{name}}}, and text in quotes."
                    )),
                }
            }
            Token::Op(op) => Err(format!("\"{op}\" needs something before it.")),
            Token::Close => Err("A ) has no ( before it.".to_owned()),
            Token::Comma => Err("A , is only for between a function's parts.".to_owned()),
        }
    }
}

// ---------------------------------------------------------------------------
// Writing it back

/// The expression as source, naming fields with `name` (which writes the
/// whole `{...}`).
pub fn print(expr: &Expr, name: &dyn Fn(FieldRef) -> String) -> String {
    let mut out = String::new();
    write(expr, name, &mut out, 0);
    out
}

fn write(expr: &Expr, name: &dyn Fn(FieldRef) -> String, out: &mut String, outer: u8) {
    match expr {
        Expr::Number(number) => {
            if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
                out.push_str(&format!("{}", *number as i64));
            } else {
                out.push_str(&format!("{number}"));
            }
        }
        Expr::Text(text) => {
            out.push('"');
            out.push_str(&text.replace('"', "\"\""));
            out.push('"');
        }
        Expr::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Expr::Field(reference) => out.push_str(&name(*reference)),
        Expr::Call(function, args) => {
            out.push_str(function);
            out.push('(');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write(arg, name, out, 0);
            }
            out.push(')');
        }
        Expr::Neg(inner) => {
            out.push('-');
            write(inner, name, out, 7);
        }
        Expr::Binary(op, left, right) => {
            let precedence = op.precedence();
            let wrap = precedence < outer;
            if wrap {
                out.push('(');
            }
            write(left, name, out, precedence);
            out.push(' ');
            out.push_str(op.symbol());
            out.push(' ');
            // Left to right: a right side of the same strength keeps its brackets.
            write(right, name, out, precedence + 1);
            if wrap {
                out.push(')');
            }
        }
    }
}

/// How a stored reference is written: `{#title}` or `{#<id>}`.
pub fn stored_name(reference: FieldRef) -> String {
    match reference {
        FieldRef::Title => "{#title}".to_owned(),
        FieldRef::Field(id) => format!("{{#{id}}}"),
    }
}

/// Reads a stored reference (`#title`, `#<id>`).
pub fn stored_reference(inside: &str) -> Result<FieldRef, String> {
    let Some(rest) = inside.strip_prefix('#') else {
        return Err(format!("{{{inside}}} isn't stored by ID."));
    };
    if rest.eq_ignore_ascii_case("title") {
        return Ok(FieldRef::Title);
    }
    rest.trim()
        .parse::<Uuid>()
        .map(FieldRef::Field)
        .map_err(|_| format!("{{{inside}}} names no field."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_name(name: &str) -> Result<FieldRef, String> {
        match name {
            "Title" => Ok(FieldRef::Title),
            other if other.starts_with('#') => stored_reference(other),
            "Price" => Ok(FieldRef::Field(Uuid::from_u128(1))),
            "Quantity" => Ok(FieldRef::Field(Uuid::from_u128(2))),
            other => Err(format!("No field {other}")),
        }
    }

    fn round_trip(source: &str) -> String {
        let expr = parse(source, &by_name).unwrap();
        print(&expr, &|reference| match reference {
            FieldRef::Title => "{Title}".to_owned(),
            FieldRef::Field(id) if id == Uuid::from_u128(1) => "{Price}".to_owned(),
            FieldRef::Field(_) => "{Quantity}".to_owned(),
        })
    }

    #[test]
    fn precedence_follows_spreadsheets() {
        assert_eq!(round_trip("1+2*3"), "1 + 2 * 3");
        assert_eq!(round_trip("(1+2)*3"), "(1 + 2) * 3");
        assert_eq!(
            round_trip("{Price}*{Quantity} & \" each\""),
            "{Price} * {Quantity} & \" each\""
        );
        assert_eq!(round_trip("1 - (2 - 3)"), "1 - (2 - 3)");
        assert_eq!(round_trip("a_b(1)").to_string(), "A_B(1)");
        assert_eq!(
            round_trip("if({Price} > 3 AND true, \"say \"\"hi\"\"\", -{Quantity})"),
            "IF({Price} > 3 and true, \"say \"\"hi\"\"\", -{Quantity})"
        );
        assert_eq!(
            round_trip("{Price} <> 2 || {Price} == 3"),
            "{Price} != 2 or {Price} = 3"
        );
        assert_eq!(round_trip("1.5e3 + .5"), "1500 + 0.5");
    }

    #[test]
    fn mistakes_are_explained() {
        for (source, part) in [
            ("", "Write a formula"),
            ("1 +", "ends too soon"),
            ("(1 + 2", "isn't closed"),
            ("\"open", "isn't closed"),
            ("{Price", "isn't closed"),
            ("Price * 2", "Put a field's name in { }"),
            ("1 2", "follows the end"),
            ("SUM(1 2)", "needs a ,"),
            ("1 = 2 = 3", "two things at a time"),
            ("{Nope}", "No field Nope"),
            ("1 $ 2", "can't be used here"),
        ] {
            let error = parse(source, &by_name).unwrap_err();
            assert!(error.contains(part), "{source}: {error}");
        }
    }

    #[test]
    fn depth_is_capped() {
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        assert!(parse(&deep, &by_name).unwrap_err().contains("levels deep"));
        let calls = format!("{}1{}", "ABS(".repeat(40), ")".repeat(40));
        assert!(parse(&calls, &by_name).unwrap_err().contains("levels deep"));
        let fine = format!("{}1{}", "(".repeat(20), ")".repeat(20));
        assert!(parse(&fine, &by_name).is_ok());
    }

    #[test]
    fn references_are_collected() {
        let expr = parse("{Price} * {Quantity} + {Price} & {Title}", &by_name).unwrap();
        assert_eq!(
            expr.references(),
            vec![
                FieldRef::Field(Uuid::from_u128(1)),
                FieldRef::Field(Uuid::from_u128(2)),
                FieldRef::Field(Uuid::from_u128(1)),
                FieldRef::Title
            ]
        );
        assert_eq!(stored_name(FieldRef::Title), "{#title}");
        assert_eq!(
            stored_reference(&format!("#{}", Uuid::from_u128(1))),
            Ok(FieldRef::Field(Uuid::from_u128(1)))
        );
    }
}

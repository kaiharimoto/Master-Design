//! A very small arithmetic expression evaluator.
//!
//! Animation manifests need to say things like `duration + stagger * (count - 1)` or
//! `pathLength`. A full scripting language would be the obvious answer and the wrong
//! one: it would mean shipping a JS engine into the exporter, the CLI and the MCP
//! server, none of which have any other reason to have one. Arithmetic over named
//! variables covers what motion actually needs.
//!
//! Grammar:
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := unary (('*' | '/' | '%') unary)*
//! unary   := '-'? primary
//! primary := number | identifier | identifier '(' args ')' | '(' expr ')'
//! ```

use crate::error::{AnimError, Result};
use std::collections::BTreeMap;

/// Variables an expression can reference.
pub type Scope = BTreeMap<String, f64>;

/// Parse and evaluate in one step.
///
/// Expressions are short and evaluated once per target at bake time, so there is no
/// benefit in caching a parsed form.
pub fn eval(source: &str, scope: &Scope) -> Result<f64> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        scope,
        source,
    };
    let value = parser.expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(AnimError::Expression {
            expr: source.to_string(),
            reason: format!("unexpected trailing input at token {}", parser.pos),
        });
    }
    if !value.is_finite() {
        return Err(AnimError::Expression {
            expr: source.to_string(),
            reason: "evaluated to a non-finite number".into(),
        });
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n = text.parse::<f64>().map_err(|_| AnimError::Expression {
                    expr: s.to_string(),
                    reason: format!("'{text}' is not a number"),
                })?;
                out.push(Token::Number(n));
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            }
            other => {
                return Err(AnimError::Expression {
                    expr: s.to_string(),
                    reason: format!("unexpected character '{other}'"),
                })
            }
        }
    }

    Ok(out)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    scope: &'a Scope,
    source: &'a str,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn fail<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(AnimError::Expression {
            expr: self.source.to_string(),
            reason: reason.into(),
        })
    }

    fn expr(&mut self) -> Result<f64> {
        let mut acc = self.term()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Token::Plus => {
                    self.pos += 1;
                    acc += self.term()?;
                }
                Token::Minus => {
                    self.pos += 1;
                    acc -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn term(&mut self) -> Result<f64> {
        let mut acc = self.unary()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Token::Star => {
                    self.pos += 1;
                    acc *= self.unary()?;
                }
                Token::Slash => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    if rhs == 0.0 {
                        return self.fail("division by zero");
                    }
                    acc /= rhs;
                }
                Token::Percent => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    if rhs == 0.0 {
                        return self.fail("modulo by zero");
                    }
                    acc %= rhs;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn unary(&mut self) -> Result<f64> {
        if self.peek() == Some(&Token::Minus) {
            self.pos += 1;
            return Ok(-self.unary()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<f64> {
        match self.bump() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::LParen) => {
                let v = self.expr()?;
                if self.bump() != Some(Token::RParen) {
                    return self.fail("expected ')'");
                }
                Ok(v)
            }
            Some(Token::Ident(name)) => {
                if self.peek() == Some(&Token::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Token::RParen) {
                        loop {
                            args.push(self.expr()?);
                            match self.peek() {
                                Some(Token::Comma) => self.pos += 1,
                                _ => break,
                            }
                        }
                    }
                    if self.bump() != Some(Token::RParen) {
                        return self.fail(format!("expected ')' closing {name}("));
                    }
                    return call(&name, &args).map_err(|reason| AnimError::Expression {
                        expr: self.source.to_string(),
                        reason,
                    });
                }
                self.scope.get(&name).copied().ok_or(()).or_else(|_| {
                    let mut known: Vec<&str> = self.scope.keys().map(|s| s.as_str()).collect();
                    known.sort();
                    self.fail(format!(
                        "unknown variable '{name}' (available: {})",
                        known.join(", ")
                    ))
                })
            }
            other => self.fail(format!("unexpected {other:?}")),
        }
    }
}

fn call(name: &str, args: &[f64]) -> std::result::Result<f64, String> {
    let arity = |n: usize| -> std::result::Result<(), String> {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                "{name}() takes {n} argument(s), got {}",
                args.len()
            ))
        }
    };

    match name {
        "abs" => arity(1).map(|_| args[0].abs()),
        "floor" => arity(1).map(|_| args[0].floor()),
        "ceil" => arity(1).map(|_| args[0].ceil()),
        "round" => arity(1).map(|_| args[0].round()),
        "sqrt" => arity(1).and_then(|_| {
            if args[0] < 0.0 {
                Err("sqrt() of a negative number".into())
            } else {
                Ok(args[0].sqrt())
            }
        }),
        "min" => {
            if args.is_empty() {
                Err("min() needs at least one argument".into())
            } else {
                Ok(args.iter().copied().fold(f64::INFINITY, f64::min))
            }
        }
        "max" => {
            if args.is_empty() {
                Err("max() needs at least one argument".into())
            } else {
                Ok(args.iter().copied().fold(f64::NEG_INFINITY, f64::max))
            }
        }
        "clamp" => arity(3).map(|_| args[0].clamp(args[1].min(args[2]), args[2].max(args[1]))),
        "lerp" => arity(3).map(|_| args[0] + (args[1] - args[0]) * args[2]),
        "pow" => arity(2).map(|_| args[0].powf(args[1])),
        other => Err(format!("unknown function '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        let mut s = Scope::new();
        s.insert("distance".into(), 32.0);
        s.insert("stagger".into(), 0.08);
        s.insert("count".into(), 4.0);
        s.insert("index".into(), 2.0);
        s
    }

    fn ev(src: &str) -> f64 {
        eval(src, &scope()).unwrap()
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("10 / 4"), 2.5);
        assert_eq!(ev("7 % 3"), 1.0);
    }

    #[test]
    fn unary_minus() {
        assert_eq!(ev("-5"), -5.0);
        assert_eq!(ev("3 - -2"), 5.0);
        assert_eq!(ev("--4"), 4.0);
    }

    #[test]
    fn variables_resolve() {
        assert_eq!(ev("distance"), 32.0);
        assert!((ev("stagger * (count - 1)") - 0.24).abs() < 1e-9);
    }

    #[test]
    fn functions_work() {
        assert_eq!(ev("max(1, 5, 3)"), 5.0);
        assert_eq!(ev("min(1, 5, 3)"), 1.0);
        assert_eq!(ev("clamp(10, 0, 4)"), 4.0);
        assert_eq!(ev("abs(0 - 7)"), 7.0);
        assert_eq!(ev("round(2.6)"), 3.0);
        assert_eq!(ev("lerp(0, 10, 0.25)"), 2.5);
        assert_eq!(ev("pow(2, 10)"), 1024.0);
    }

    #[test]
    fn an_unknown_variable_lists_what_is_available() {
        let err = eval("wobble * 2", &scope()).unwrap_err().to_string();
        assert!(err.contains("wobble"), "got {err}");
        assert!(
            err.contains("distance"),
            "the error should list valid names: {err}"
        );
    }

    #[test]
    fn malformed_input_is_rejected() {
        for bad in ["1 +", "(1 + 2", "1 2", "* 3", "1 / 0", "nope()", "$"] {
            assert!(eval(bad, &scope()).is_err(), "'{bad}' should not evaluate");
        }
    }

    #[test]
    fn an_empty_expression_is_an_error() {
        assert!(eval("", &scope()).is_err());
        assert!(eval("   ", &scope()).is_err());
    }
}

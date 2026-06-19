use anyhow::anyhow;
use logos::Logos;
use pact_models::matchingrules::expressions::{parse_matcher_def, MatchingRuleDefinition};
use prost_types::value::Kind;

#[derive(Logos, Debug, PartialEq, Clone)]
enum FieldToken {
    #[token("[")]
    LBracket,

    #[token("]")]
    RBracket,

    #[token("*")]
    Asterisk,

    #[token(".")]
    Dot,

    #[regex("[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Word(String),

    #[regex(r"[ \t\n\f]+", logos::skip)]
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldKey {
    pub field: String,
    pub event_type: Option<String>,
}

impl FieldKey {
    pub fn path(&self) -> String {
        match &self.event_type {
            Some(et) => format!("{}.{}.*", self.field, et),
            None if self.field == "event" || self.field == "retry" => self.field.clone(),
            None => format!("{}.*", self.field),
        }
    }

    #[allow(dead_code)]
    pub fn is_typed(&self) -> bool {
        self.event_type.is_some()
    }
}

fn next_token(lex: &mut logos::Lexer<FieldToken>) -> Option<FieldToken> {
    loop {
        match lex.next() {
            Some(Ok(token)) => return Some(token),
            Some(Err(_)) => continue,
            None => return None,
        }
    }
}

pub(crate) fn parse_field(s: &str) -> anyhow::Result<FieldKey> {
    let s = s.trim();

    if s == "event" {
        return Ok(FieldKey {
            field: "event".to_string(),
            event_type: None,
        });
    }
    if s == "retry" {
        return Ok(FieldKey {
            field: "retry".to_string(),
            event_type: None,
        });
    }

    let mut lex = FieldToken::lexer(s);

    let first = next_token(&mut lex)
        .ok_or_else(|| anyhow!("Empty field definition: '{}'", s))?;
    if let FieldToken::Word(ref word) = first {
        let field = word.clone();

        match next_token(&mut lex) {
            Some(FieldToken::LBracket) => {
                let et = next_token(&mut lex)
                    .ok_or_else(|| anyhow!("Expected event type in '{}'", s))?;
                match et {
                    FieldToken::Asterisk => {
                        // data[*] means "all events" (no specific type)
                        let closing = next_token(&mut lex);
                        if closing != Some(FieldToken::RBracket) {
                            return Err(anyhow!("Expected ']' in field definition: '{}'", s));
                        }
                        Ok(FieldKey {
                            field,
                            event_type: None,
                        })
                    }
                    FieldToken::Word(ref et) => {
                        let closing = next_token(&mut lex);
                        if closing != Some(FieldToken::RBracket) {
                            return Err(anyhow!("Expected ']' in field definition: '{}'", s));
                        }
                        // Check for optional [*] suffix
                        let next = next_token(&mut lex);
                        if next == Some(FieldToken::LBracket) {
                            let wildcard = next_token(&mut lex);
                            if wildcard != Some(FieldToken::Asterisk) {
                                return Err(anyhow!("Expected '[*]' in field definition: '{}'", s));
                            }
                            let closing = next_token(&mut lex);
                            if closing != Some(FieldToken::RBracket) {
                                return Err(anyhow!("Expected ']' in field definition: '{}'", s));
                            }
                        }
                        Ok(FieldKey {
                            field,
                            event_type: Some(et.clone()),
                        })
                    }
                    _ => Err(anyhow!("Expected event type in field definition: '{}'", s)),
                }
            }
            Some(FieldToken::Dot) => {
                // Handle dot notation: field.type.*
                let et = next_token(&mut lex)
                    .ok_or_else(|| anyhow!("Expected event type after '.' in '{}'", s))?;
                if let FieldToken::Word(ref et) = et {
                    // Check for trailing .*
                    let dot = next_token(&mut lex);
                    if dot == Some(FieldToken::Dot) {
                        let asterisk = next_token(&mut lex);
                        if asterisk != Some(FieldToken::Asterisk) {
                            return Err(anyhow!(
                                "Expected '*' after '.' in field definition: '{}'",
                                s
                            ));
                        }
                    }
                    Ok(FieldKey {
                        field,
                        event_type: Some(et.clone()),
                    })
                } else if let FieldToken::Asterisk = et {
                    // field.* means all events (no specific type)
                    Ok(FieldKey {
                        field,
                        event_type: None,
                    })
                } else {
                    Err(anyhow!("Expected event type after '.' in '{}'", s))
                }
            }
            None => Ok(FieldKey {
                field,
                event_type: None,
            }),
            _ => Err(anyhow!("Invalid field definition: '{}'", s)),
        }
    } else {
        Err(anyhow!("Expected field name in '{}'", s))
    }
}

pub(crate) fn parse_value(v: &prost_types::Value) -> anyhow::Result<MatchingRuleDefinition> {
    if let Some(kind) = &v.kind {
        match kind {
            Kind::StringValue(s) => parse_matcher_def(s),
            Kind::NullValue(_) => Err(anyhow!("Null is not a valid value definition")),
            Kind::NumberValue(_) => Err(anyhow!("Number is not a valid value definition")),
            Kind::BoolValue(_) => Err(anyhow!("Bool is not a valid value definition")),
            Kind::StructValue(_) => Err(anyhow!("Struct is not a valid value definition")),
            Kind::ListValue(_) => Err(anyhow!("List is not a valid value definition")),
        }
    } else {
        Err(anyhow!("Not a valid value definition (missing value)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_field_event() {
        let key = parse_field("event").unwrap();
        assert_eq!(key.field, "event");
        assert!(key.event_type.is_none());
        assert_eq!(key.path(), "event");
    }

    #[test]
    fn test_parse_field_retry() {
        let key = parse_field("retry").unwrap();
        assert_eq!(key.field, "retry");
        assert!(key.event_type.is_none());
        assert_eq!(key.path(), "retry");
    }

    #[test]
    fn test_parse_field_data_bracket() {
        let key = parse_field("data[count]").unwrap();
        assert_eq!(key.field, "data");
        assert_eq!(key.event_type, Some("count".to_string()));
        assert_eq!(key.path(), "data.count.*");
    }

    #[test]
    fn test_parse_field_data_bracket_wildcard() {
        let key = parse_field("id[user][*]").unwrap();
        assert_eq!(key.field, "id");
        assert_eq!(key.event_type, Some("user".to_string()));
        assert_eq!(key.path(), "id.user.*");
    }

    #[test]
    fn test_parse_field_data_wildcard() {
        let key = parse_field("data[*]").unwrap();
        assert_eq!(key.field, "data");
        assert!(key.event_type.is_none());
        assert_eq!(key.path(), "data.*");
    }

    #[test]
    fn test_parse_field_dot_notation() {
        let key = parse_field("data.count.*").unwrap();
        assert_eq!(key.field, "data");
        assert_eq!(key.event_type, Some("count".to_string()));
        assert_eq!(key.path(), "data.count.*");
    }

    #[test]
    fn test_parse_field_id_wildcard() {
        let key = parse_field("id.*").unwrap();
        assert_eq!(key.field, "id");
        assert!(key.event_type.is_none());
        assert_eq!(key.path(), "id.*");
    }
}

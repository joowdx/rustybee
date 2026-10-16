//! Inbound SMS filters (spec 002). The device applies them; the server stores,
//! edits and ships them in `config`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    Allow,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Sender,
    Body,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Equals,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    pub value: String,
    #[serde(default = "yes")]
    pub ignore_case: bool,
}

fn yes() -> bool {
    true
}

pub const MAX_RULES: usize = 100;
pub const MAX_VALUE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Filters {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Filters {
    /// Limits shared by the web form and the device's `set_filters`.
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.len() > MAX_RULES {
            return Err(format!("at most {MAX_RULES} rules"));
        }
        for (i, r) in self.rules.iter().enumerate() {
            let n = r.value.chars().count();
            if r.value.trim().is_empty() || n > MAX_VALUE_CHARS {
                return Err(format!(
                    "rule {}: value must be 1 to {MAX_VALUE_CHARS} characters",
                    i + 1
                ));
            }
        }
        Ok(())
    }
}

impl Rule {
    fn matches_text(&self, text: &str) -> bool {
        let (t, v) = if self.ignore_case {
            (text.to_lowercase(), self.value.to_lowercase())
        } else {
            (text.to_string(), self.value.clone())
        };
        match self.op {
            Op::Equals => t == v,
            Op::Contains => t.contains(&v),
            Op::StartsWith => t.starts_with(&v),
            Op::EndsWith => t.ends_with(&v),
        }
    }

    pub fn matches(&self, sender: &str, body: &str) -> bool {
        match self.field {
            Field::Sender => self.matches_text(sender),
            Field::Body => self.matches_text(body),
            Field::Any => self.matches_text(sender) || self.matches_text(body),
        }
    }
}

impl Filters {
    /// Whether an inbound SMS should be forwarded.
    pub fn passes(&self, sender: &str, body: &str) -> bool {
        let any = || self.rules.iter().any(|r| r.matches(sender, body));
        match self.mode {
            Mode::Off => true,
            Mode::Allow => any(),
            Mode::Block => !any(),
        }
    }
}

impl Field {
    pub fn as_str(self) -> &'static str {
        match self {
            Field::Sender => "sender",
            Field::Body => "body",
            Field::Any => "any",
        }
    }
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Equals => "equals",
            Op::Contains => "contains",
            Op::StartsWith => "starts_with",
            Op::EndsWith => "ends_with",
        }
    }
}

impl Rule {
    /// One row of the web rule editor. A row with no text is unused and gives `None`.
    pub fn from_row(
        field: &str,
        op: &str,
        value: &str,
        ignore_case: bool,
    ) -> Result<Option<Rule>, String> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let field = from_str(field).ok_or(format!("unknown field {field:?}"))?;
        let op = from_str(op).ok_or(format!("unknown match {op:?}"))?;
        Ok(Some(Rule {
            field,
            op,
            value: value.to_string(),
            ignore_case,
        }))
    }
}

fn from_str<T: for<'de> Deserialize<'de>>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(field: Field, op: Op, value: &str) -> Rule {
        Rule {
            field,
            op,
            value: value.into(),
            ignore_case: true,
        }
    }

    #[test]
    fn mode_off_forwards_everything() {
        let f = Filters {
            mode: Mode::Off,
            rules: vec![rule(Field::Body, Op::Contains, "x")],
        };
        assert!(f.passes("a", "x"));
    }

    #[test]
    fn allow_and_block() {
        let rules = vec![
            rule(Field::Sender, Op::StartsWith, "+1900"),
            rule(Field::Body, Op::Contains, "PROMO"),
        ];
        let allow = Filters {
            mode: Mode::Allow,
            rules: rules.clone(),
        };
        let block = Filters {
            mode: Mode::Block,
            rules,
        };
        assert!(allow.passes("+19005551", "hi"));
        assert!(allow.passes("+1555", "big promo today"));
        assert!(!allow.passes("+1555", "hello"));
        assert!(!block.passes("+19005551", "hi"));
        assert!(block.passes("+1555", "hello"));
    }

    #[test]
    fn ops_and_case() {
        let mut r = rule(Field::Any, Op::Equals, "Bank");
        assert!(r.matches("bank", ""));
        r.ignore_case = false;
        assert!(!r.matches("bank", ""));
        assert!(rule(Field::Body, Op::EndsWith, "stop").matches("", "reply STOP"));
        assert!(!rule(Field::Sender, Op::EndsWith, "stop").matches("", "reply STOP"));
    }

    #[test]
    fn rule_rows() {
        let r = Rule::from_row("sender", "starts_with", " +63917 ", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            (r.field, r.op, r.value.as_str(), r.ignore_case),
            (Field::Sender, Op::StartsWith, "+63917", false)
        );
        assert_eq!(Rule::from_row("body", "contains", "  ", true), Ok(None));
        assert!(Rule::from_row("sender", "like", "x", true).is_err());
        assert!(Rule::from_row("subject", "equals", "x", true).is_err());
        assert_eq!(Op::EndsWith.as_str(), "ends_with");
    }
}

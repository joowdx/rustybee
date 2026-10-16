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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Filters {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub rules: Vec<Rule>,
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

    /// Rules as editable text: one `field op value` per line.
    pub fn rules_text(&self) -> String {
        self.rules
            .iter()
            .map(|r| format!("{} {} {}", enum_str(&r.field), enum_str(&r.op), r.value))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Parse the editor format back. Every rule gets the same `ignore_case`.
    pub fn parse_rules_text(text: &str, ignore_case: bool) -> Result<Vec<Rule>, String> {
        let mut rules = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.splitn(3, char::is_whitespace);
            let (Some(f), Some(o), Some(v)) = (it.next(), it.next(), it.next()) else {
                return Err(format!("line {}: expected `field op value`", i + 1));
            };
            let field: Field = from_str(f).ok_or(format!("line {}: unknown field {f:?}", i + 1))?;
            let op: Op = from_str(o).ok_or(format!("line {}: unknown op {o:?}", i + 1))?;
            rules.push(Rule {
                field,
                op,
                value: v.trim().to_string(),
                ignore_case,
            });
        }
        Ok(rules)
    }
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
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
    fn rules_text_round_trip() {
        let rules = vec![
            rule(Field::Sender, Op::StartsWith, "+1900"),
            rule(Field::Body, Op::Contains, "two words"),
        ];
        let f = Filters {
            mode: Mode::Block,
            rules: rules.clone(),
        };
        let text = f.rules_text();
        assert_eq!(text, "sender starts_with +1900\nbody contains two words");
        assert_eq!(Filters::parse_rules_text(&text, true).unwrap(), rules);
        assert!(Filters::parse_rules_text("sender like x", true).is_err());
        assert!(Filters::parse_rules_text("sender", true).is_err());
    }
}

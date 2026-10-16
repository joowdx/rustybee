//! Phone number normalisation (spec 004).

/// Trim, strip spaces, `-`, `(`, `)` and `.`, rewrite a local Philippine
/// mobile `09XXXXXXXXX` to `+639XXXXXXXXX`, then require `^\+?[0-9]{3,20}$`.
pub fn normalize(input: &str) -> Result<String, String> {
    let cleaned: String = input
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '.'))
        .collect();
    let cleaned = match cleaned.strip_prefix('0') {
        Some(rest)
            if cleaned.len() == 11
                && rest.starts_with('9')
                && rest.bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("+63{rest}")
        }
        _ => cleaned,
    };
    let digits = cleaned.strip_prefix('+').unwrap_or(&cleaned);
    if (3..=20).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
        Ok(cleaned)
    } else {
        Err(format!("{:?} is not a valid phone number", input.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_004_06_accepts_formatted_number() {
        assert_eq!(normalize("+1 (555) 123-4567").unwrap(), "+15551234567");
        assert_eq!(normalize(" 555.123.4567 ").unwrap(), "5551234567");
    }

    #[test]
    fn ac_004_07_local_ph_mobile_gets_country_code() {
        assert_eq!(normalize("0917 123 4567").unwrap(), "+639171234567");
        assert_eq!(normalize("09171234567").unwrap(), "+639171234567");
        assert_eq!(normalize("0917123456").unwrap(), "0917123456");
        assert_eq!(normalize("091712345678").unwrap(), "091712345678");
        assert_eq!(normalize("08171234567").unwrap(), "08171234567");
        assert_eq!(normalize("+639171234567").unwrap(), "+639171234567");
    }

    #[test]
    fn ac_004_06_rejects_garbage() {
        assert!(normalize("abc").is_err());
        assert!(normalize("12").is_err());
        assert!(normalize("+").is_err());
        assert!(normalize("++123").is_err());
        assert!(normalize(&"1".repeat(21)).is_err());
        assert!(normalize("123").is_ok());
    }
}

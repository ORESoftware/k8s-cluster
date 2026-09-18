//! Input validation for admin form posts.
//!
//! These checks run **before** we touch the service layer so the user
//! gets a fast, specific error and we keep junk out of the audit log.
//! The service layer still performs its own validation (defense in
//! depth) — these helpers exist to fail fast and to mirror what the
//! schema actually accepts.

/// Validate a tenant slug. The schema column is `citext` (case-
/// insensitive text) with no uniqueness constraint, but we want a tight
/// shape so slugs are predictable URL components: lowercase ASCII
/// letters, digits, dashes; must start with a letter; 3..=40 chars.
pub fn slug(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.len() < 3 || s.len() > 40 {
        return Err("slug must be 3..=40 characters");
    }
    let mut chars = s.chars();
    let first = chars.next().ok_or("slug must not be empty")?;
    if !first.is_ascii_lowercase() {
        return Err("slug must start with a lowercase ASCII letter");
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err("slug may contain only lowercase letters, digits, and dashes");
        }
    }
    if s.contains("--") {
        return Err("slug may not contain consecutive dashes");
    }
    if s.ends_with('-') {
        return Err("slug may not end with a dash");
    }
    Ok(())
}

/// Validate a free-form display name. Trims, length 1..=120, no control
/// characters (defeats CRLF-injection and stray nulls without trying to
/// be a full sanitizer — Maud handles HTML escaping for us).
pub fn display_name(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.is_empty() {
        return Err("display name must not be empty");
    }
    if s.chars().count() > 120 {
        return Err("display name must be 120 characters or fewer");
    }
    if s.chars().any(char::is_control) {
        return Err("display name must not contain control characters");
    }
    Ok(())
}

/// Validate an ISO 3166-1 alpha-2 country code (two ASCII letters).
pub fn country_code(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.len() != 2 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("country_code must be a two-letter ISO 3166-1 code");
    }
    Ok(())
}

/// Validate a US state code when provided. Two ASCII letters.
pub fn us_state(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.len() != 2 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("us_state must be a two-letter postal abbreviation");
    }
    Ok(())
}

/// Validate an ISO 4217 currency code (three ASCII letters).
pub fn currency_code(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.len() != 3 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("currency must be a three-letter ISO 4217 code");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_accepts_canonical_shapes() {
        for ok in ["acm", "dancingdragons", "acme-co", "a1-b2-c3", "dragon42"] {
            slug(ok).unwrap_or_else(|e| panic!("expected ok for {ok:?}: {e}"));
        }
    }

    #[test]
    fn slug_rejects_bad_shapes() {
        for (bad, _why) in [
            ("", "empty"),
            ("ab", "too short"),
            ("AAA", "uppercase"),
            ("acme_co", "underscore"),
            ("-acme", "leading dash"),
            ("acme-", "trailing dash"),
            ("acme--co", "double dash"),
            ("acme.co", "dot"),
            ("1acme", "leading digit"),
            ("acme co", "space"),
            ("acme/co", "slash"),
            ("\u{1F4A9}-acme", "non-ascii"),
            (&"a".repeat(41), "too long"),
        ] {
            assert!(slug(bad).is_err(), "expected err for {bad:?}");
        }
    }

    #[test]
    fn display_name_basic() {
        assert!(display_name("Acme Co.").is_ok());
        assert!(display_name(" trim me ").is_ok());
        assert!(display_name("").is_err());
        assert!(display_name("   ").is_err());
        assert!(display_name("hello\u{0000}world").is_err());
        assert!(display_name("line1\nline2").is_err());
        assert!(display_name(&"a".repeat(121)).is_err());
        assert!(display_name(&"a".repeat(120)).is_ok());
    }

    #[test]
    fn country_code_basic() {
        assert!(country_code("US").is_ok());
        assert!(country_code("us").is_ok());
        assert!(country_code("USA").is_err());
        assert!(country_code("U1").is_err());
        assert!(country_code("").is_err());
    }

    #[test]
    fn us_state_basic() {
        assert!(us_state("CA").is_ok());
        assert!(us_state("Ca").is_ok());
        assert!(us_state("California").is_err());
        assert!(us_state("C1").is_err());
    }

    #[test]
    fn currency_code_basic() {
        assert!(currency_code("USD").is_ok());
        assert!(currency_code("usd").is_ok());
        assert!(currency_code("US").is_err());
        assert!(currency_code("USDX").is_err());
        assert!(currency_code("US1").is_err());
    }
}

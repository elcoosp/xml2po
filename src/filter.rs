//! Heuristics that decide whether a text node is human-readable content
//! worth translating.
//!
//! The filter is deliberately conservative and side-effect free: it only
//! rejects strings that are *provably* mechanical (URLs, e-mail addresses,
//! hex colors, pure numbers/symbols, lone placeholders, whitespace).

/// Returns `true` when the trimmed text looks like translatable prose.
///
/// Rejected categories:
/// - URLs and any string containing a scheme separator (`://`)
/// - e-mail-like tokens (a token containing `@` followed by a dot)
/// - `data:` URIs and hex color literals
/// - strings made only of placeholders (`{{name}}`, `{0}`, `%s`, …)
/// - whitespace-only strings and strings without alphabetic characters
///
/// Strings containing translation placeholders *plus* prose (for example
/// `Hello {{name}}`) are accepted.
pub fn is_translatable_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.chars().any(char::is_alphabetic) {
        return false;
    }
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("data:")
        || trimmed.contains("://")
    {
        return false;
    }
    if is_hex_color(trimmed) {
        return false;
    }
    // E-mail heuristic: reject when some whitespace-separated token looks
    // like `name@host.tld`, unless placeholders are involved.
    if !trimmed.contains("{{") && trimmed.split_whitespace().any(is_email_like) {
        return false;
    }
    if is_only_placeholders(trimmed) {
        return false;
    }
    true
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
fn is_hex_color(s: &str) -> bool {
    s.strip_prefix('#').is_some_and(|body| {
        let len = body.len();
        matches!(len, 3 | 4 | 6 | 8) && body.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// `user@example.com`, `a.b+c@host.co.uk`, …
fn is_email_like(token: &str) -> bool {
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// Matches gettext/c-style placeholders: `%s`, `%d`, `%1$s`, `%n$`, `%@`,
/// `{name}`, `{0}`, `{{name}}`.
fn is_placeholder_token(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    if let Some(rest) = token.strip_prefix("{{") {
        return rest.strip_suffix("}}").is_some();
    }
    if let Some(rest) = token.strip_prefix('{') {
        return rest.strip_suffix('}').is_some();
    }
    let Some(rest) = token.strip_prefix('%') else {
        return false;
    };
    if rest == "@" || rest == "%" {
        return true;
    }
    // `%s`, `%d`, `%f`, `%1$s`, `%10$d`
    let mut chars = rest.chars();
    let mut digits = 0;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if c == '$' {
            return digits > 0;
        } else {
            return matches!(c, 's' | 'd' | 'f' | 'i' | 'g') && chars.next().is_none();
        }
    }
    false
}

/// `true` when every whitespace-separated token is a placeholder or
/// punctuation, i.e. there is nothing left to translate.
fn is_only_placeholders(text: &str) -> bool {
    let meaningful = text
        .split_whitespace()
        .filter(|t| !is_placeholder_token(t) && t.chars().any(char::is_alphanumeric));
    meaningful.count() == 0 && text.split_whitespace().any(is_placeholder_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_prose() {
        for s in [
            "Hello World",
            "Welcome to our service",
            "Click here",
            "Hello {{name}}, you have {{count}} messages",
            "Hello {0}, welcome back",
            "Loaded %s items",
            "Total: %1$s",
            "Save 100% today",
            "Résumé & Co — déjà vu",
        ] {
            assert!(is_translatable_text(s), "{s:?} should be translatable");
        }
    }

    #[test]
    fn rejects_urls_and_uris() {
        for s in [
            "https://example.com",
            "http://localhost:3000",
            "ftp://example.com/file",
            "data:image/png;base64,AAAA",
            "See://nothing",
        ] {
            assert!(!is_translatable_text(s), "{s:?} should be rejected");
        }
    }

    #[test]
    fn rejects_emails() {
        for s in [
            "user@example.com",
            "test.email@domain.co.uk",
            "write to me@you.org now",
        ] {
            assert!(!is_translatable_text(s), "{s:?} should be rejected");
        }
        // Placeholders with dots/emails stay acceptable.
        assert!(is_translatable_text("{{ user.email }}"));
    }

    #[test]
    fn rejects_empty_whitespace_and_symbols() {
        for s in [
            "",
            "   ",
            "\t\n",
            "12345",
            "!@#$%",
            "123-456-7890",
            "-",
            "@",
        ] {
            assert!(!is_translatable_text(s), "{s:?} should be rejected");
        }
    }

    #[test]
    fn rejects_hex_colors() {
        for s in ["#fff", "#ffff", "#ffffff", "#ffffffff", "#a1b2c3"] {
            assert!(!is_translatable_text(s), "{s:?} should be rejected");
        }
        assert!(is_translatable_text("#hashtag prose"));
        assert!(!is_translatable_text("#12345"));
    }

    #[test]
    fn rejects_pure_placeholders() {
        for s in [
            "{{name}}",
            "{{name}} {{count}}",
            "%s %d",
            "{0} {1}",
            "%@",
            "%%",
        ] {
            assert!(!is_translatable_text(s), "{s:?} should be rejected");
        }
        assert!(is_translatable_text("%s new messages"));
        assert!(is_translatable_text("{0} found"));
        assert!(is_translatable_text("%1$s of %2$d"));
    }

    #[test]
    fn placeholder_token_edge_cases() {
        assert!(is_placeholder_token("{{ x }}"));
        assert!(!is_placeholder_token("{{"));
        assert!(!is_placeholder_token("x{{}}"));
        assert!(is_placeholder_token("{42}"));
        assert!(!is_placeholder_token("{42"));
        assert!(is_placeholder_token("%10$d"));
        assert!(is_placeholder_token("%@"));
        assert!(!is_placeholder_token("%"));
        assert!(!is_placeholder_token("%1"));
        assert!(!is_placeholder_token("abc"));
        assert!(!is_placeholder_token(""));
    }

    #[test]
    fn email_like_tokens() {
        assert!(is_email_like("a@b.co"));
        assert!(is_email_like("a.b+c@host.co.uk"));
        assert!(!is_email_like("@b.co"));
        assert!(!is_email_like("a@"));
        assert!(!is_email_like("a@.co"));
        assert!(!is_email_like("a@co."));
        assert!(!is_email_like("plain"));
    }
}

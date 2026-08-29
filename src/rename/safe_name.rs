/// Normalize an arbitrary string into a valid Lua identifier.
///
/// Lua identifiers are ASCII-only: `[A-Za-z_][A-Za-z0-9_]*`, excluding keywords.
///
/// - Collapses separator runs (`.`, ` `, `-`) by uppercasing the next letter.
/// - `_` is identifier-continue, NOT a separator.
/// - Strips chars that are neither identifier-continue nor separators
///   (including any non-ASCII).
/// - Prepends `_` if the result is empty, starts with a digit, or is a keyword.
pub fn to_identifier(raw: &str) -> String {
    if raw.is_empty() {
        return "_".to_string();
    }

    let mut result = String::with_capacity(raw.len());
    let mut capitalize_next = false;

    for ch in raw.chars() {
        if ch == '.' || ch == ' ' || ch == '-' {
            // Separator: set flag only if we have content to continue from.
            if !result.is_empty() {
                capitalize_next = true;
            }
        } else if ch == '_' || ch.is_ascii_alphanumeric() {
            if capitalize_next {
                capitalize_next = false;
                result.push(ch.to_ascii_uppercase());
            } else {
                result.push(ch);
            }
        }
        // All other chars dropped silently.
    }

    if result.is_empty() {
        return "_".to_string();
    }

    // Prepend `_` if the first char is a digit.
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    // Prepend `_` if the result is a keyword.
    if is_reserved_word(&result) {
        result.insert(0, '_');
    }

    result
}

/// Keywords across Lua 5.1–5.4 and LuaJIT. `goto` is only reserved from 5.2 on
/// but is treated as reserved everywhere so output parses under any version.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Returns true iff `name` is a Lua keyword and cannot be used as a binding name.
pub fn is_reserved_word(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::super::test_dsl::to_identifier_of;

    // --- to_identifier ---

    #[test]
    fn preserves_simple_name() {
        to_identifier_of("foo").is("foo");
    }

    #[test]
    fn strips_dot_and_camel_cases() {
        to_identifier_of("self.kLength").is("selfKLength");
    }

    #[test]
    fn strips_space_and_camel_cases() {
        to_identifier_of("foo bar").is("fooBar");
    }

    #[test]
    fn strips_dash_and_camel_cases() {
        to_identifier_of("foo-bar-baz").is("fooBarBaz");
    }

    #[test]
    fn collapses_consecutive_separators() {
        to_identifier_of("a..b").is("aB");
    }

    #[test]
    fn drops_unknown_punctuation() {
        to_identifier_of("foo!?bar").is("foobar");
    }

    #[test]
    fn drops_dollar_sign() {
        to_identifier_of("$value").is("value");
    }

    #[test]
    fn drops_non_ascii_letters() {
        to_identifier_of("contagem_até").is("contagem_at");
    }

    #[test]
    fn empty_input_returns_underscore() {
        to_identifier_of("").is("_");
    }

    #[test]
    fn all_stripped_returns_underscore() {
        to_identifier_of("!!??").is("_");
    }

    #[test]
    fn leading_digit_prepends_underscore() {
        to_identifier_of("123abc").is("_123abc");
    }

    #[test]
    fn leading_separator_does_not_capitalize() {
        to_identifier_of(".foo").is("foo");
    }

    #[test]
    fn underscore_is_not_a_separator() {
        to_identifier_of("foo_bar").is("foo_bar");
    }

    #[test]
    fn keyword_end() {
        to_identifier_of("end").is("_end");
    }

    #[test]
    fn keyword_function() {
        to_identifier_of("function").is("_function");
    }

    #[test]
    fn keyword_goto() {
        to_identifier_of("goto").is("_goto");
    }

    #[test]
    fn keyword_nil() {
        to_identifier_of("nil").is("_nil");
    }

    #[test]
    fn keyword_is_case_sensitive() {
        to_identifier_of("End").is("End");
    }

    // --- is_reserved_word ---

    #[test]
    fn reserved_local() {
        to_identifier_of("local").is_reserved();
    }

    #[test]
    fn not_reserved_self() {
        // `self` is an ordinary identifier in Lua.
        to_identifier_of("self").is_not_reserved();
    }

    #[test]
    fn not_reserved_class() {
        to_identifier_of("class").is_not_reserved();
    }
}

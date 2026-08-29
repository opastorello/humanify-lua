use super::scope::{BindingId, ScopeModel};

/// Split a trailing run of ASCII decimal digits off an identifier.
///
/// `"index3"` -> `("index", Some(3))`, `"iterator"` -> `("iterator", None)`.
/// An all-digit string (impossible for a valid identifier) yields `None`.
fn split_numeric_suffix(name: &str) -> (&str, Option<u64>) {
    let bytes = name.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == 0 || i == bytes.len() {
        (name, None)
    } else {
        (&name[..i], name[i..].parse::<u64>().ok())
    }
}

/// Resolve `base` to a free name given an `is_taken` predicate, appending or
/// **incrementing** a numeric suffix.
///
/// If `base` already ends in a number, that number is incremented rather than a
/// new one appended — so `userAccountManager2` becomes `userAccountManager3`
/// (not `userAccountManager22`), and `index3` becomes `index4`. Names without
/// a trailing number start at `2` (`index` -> `index2` -> `index3`).
///
/// Returns the resolved name and whether any suffixing was needed.
pub(crate) fn dedupe<F: Fn(&str) -> bool>(base: &str, is_taken: F) -> (String, bool) {
    if !is_taken(base) {
        return (base.to_string(), false);
    }
    let (stem, start) = split_numeric_suffix(base);
    let mut n = start.map(|x| x.saturating_add(1)).unwrap_or(2);
    loop {
        let candidate = format!("{stem}{n}");
        if !is_taken(&candidate) {
            return (candidate, true);
        }
        n += 1;
    }
}

/// Returns true if renaming `binding` to `candidate` would change what any
/// identifier in the chunk resolves to.
///
/// Let `S` be the scope that declares `binding`. The rename is unsafe when:
///
/// 1. **Another binding named `candidate` lives in `S` or a scope nested in
///    `S`.** In the same scope the two declarations would shadow each other;
///    in a nested scope, references to `binding` inside that nested scope
///    would silently rebind to the inner declaration (renaming outer `a` to
///    `x` in `local a; local function f() local x; return a end`).
/// 2. **An identifier named `candidate` that resolves to something else
///    (an outer local or a global) occurs inside `S`.** After the rename it
///    would be captured by `binding` instead (`local a = 1; print(x)` with
///    `a -> x` turns the global `x` into the local).
///
/// Names in *disjoint* scopes (sibling functions) may freely be reused, so
/// generic names like `index` don't accrue suffixes across the file.
pub(crate) fn collides(model: &ScopeModel, binding: BindingId, candidate: &str) -> bool {
    let scope = model.bindings[binding].scope;
    let span = model.scopes[scope].span;

    for (id, other) in model.bindings.iter().enumerate() {
        if id == binding || other.name != candidate {
            continue;
        }
        if model.is_within(other.scope, scope) {
            return true;
        }
        if other.refs.iter().any(|r| span.contains(r.start)) {
            return true;
        }
    }

    model
        .free_refs
        .iter()
        .any(|free| free.name == candidate && span.contains(free.span.start))
}

#[cfg(test)]
mod tests {
    use super::super::test_dsl::{model_of, renaming};
    use super::*;

    // --- dedupe ---

    #[test]
    fn dedupe_free_name_unchanged() {
        assert_eq!(dedupe("index", |_| false), ("index".to_string(), false));
    }

    #[test]
    fn dedupe_appends_two_first() {
        assert_eq!(
            dedupe("index", |n| n == "index"),
            ("index2".to_string(), true)
        );
    }

    #[test]
    fn dedupe_increments_existing_number() {
        assert_eq!(
            dedupe("index3", |n| n == "index3"),
            ("index4".to_string(), true)
        );
    }

    #[test]
    fn dedupe_skips_taken_candidates() {
        let taken = ["index", "index2", "index3"];
        assert_eq!(
            dedupe("index", |n| taken.contains(&n)),
            ("index4".to_string(), true)
        );
    }

    #[test]
    fn split_numeric_suffix_all_digits_is_not_split() {
        assert_eq!(split_numeric_suffix("123"), ("123", None));
    }

    // --- collides ---

    #[test]
    fn free_name_does_not_collide() {
        assert!(!renaming(model_of("local a = 1"), "a")
            .to("count")
            .collides());
    }

    #[test]
    fn own_name_does_not_collide() {
        assert!(!renaming(model_of("local a = 1"), "a").to("a").collides());
    }

    #[test]
    fn same_scope_binding_collides() {
        assert!(renaming(model_of("local a, b = 1, 2"), "a")
            .to("b")
            .collides());
    }

    #[test]
    fn nested_scope_binding_collides() {
        assert!(renaming(
            model_of("local a = 1\nlocal function f() local x = 2 return a end"),
            "a"
        )
        .to("x")
        .collides());
    }

    #[test]
    fn global_referenced_inside_scope_collides() {
        assert!(renaming(model_of("local a = 1\nprint(a)"), "a")
            .to("print")
            .collides());
    }

    #[test]
    fn outer_local_referenced_inside_scope_collides() {
        assert!(renaming(
            model_of("local x = 1\nlocal function f() local a = 2 return a + x end"),
            "a"
        )
        .to("x")
        .collides());
    }

    #[test]
    fn outer_local_not_referenced_inside_scope_may_be_shadowed() {
        assert!(!renaming(
            model_of("local x = 1\nlocal function f() local a = 2 return a end"),
            "a"
        )
        .to("x")
        .collides());
    }

    #[test]
    fn sibling_scope_binding_does_not_collide() {
        assert!(!renaming(
            model_of("local function f() local a = 1 end\nlocal function g() local b = 2 end"),
            "a"
        )
        .to("b")
        .collides());
    }

    #[test]
    fn global_referenced_outside_scope_does_not_collide() {
        assert!(!renaming(
            model_of("local function f() local a = 1 return a end\nprint(f())"),
            "a"
        )
        .to("print")
        .collides());
    }

    #[test]
    fn implicit_self_collides() {
        assert!(renaming(model_of("function T:m(a) return a end"), "a")
            .to("self")
            .collides());
    }
}

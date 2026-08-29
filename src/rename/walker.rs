use full_moon::LuaVersion;

use super::collision;
use super::scope::{ScopeModel, Span};
use super::{NoopRenameObserver, RenameError, RenameObserver, Renamer};

pub fn rename_all_identifiers(
    source: &str,
    renamer: &mut dyn Renamer,
    context_size: usize,
) -> Result<String, RenameError> {
    rename_all_identifiers_with_observer(source, renamer, context_size, &mut NoopRenameObserver)
}

pub fn rename_all_identifiers_with_observer(
    source: &str,
    renamer: &mut dyn Renamer,
    context_size: usize,
    observer: &mut dyn RenameObserver,
) -> Result<String, RenameError> {
    if source.is_empty() {
        observer.identifiers_found(0);
        return Ok(String::new());
    }

    // `LuaVersion::new()` enables every version compiled in (5.1–5.4 + LuaJIT).
    // `with_cfxlua()` additionally accepts CitizenFX/FiveM extensions (`+=`-style
    // compound assignment, `?.` safe navigation, `.a` table-constructor shorthand,
    // `local x in y`) on top of that, so the parser accepts the union of all of
    // their syntaxes.
    let parsed = full_moon::parse_fallible(source, LuaVersion::new().with_cfxlua());
    if !parsed.errors().is_empty() {
        let msg = parsed
            .errors()
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(RenameError::Parse(msg));
    }

    let mut model = ScopeModel::build(parsed.ast(), source.len());

    // Process largest scope first (so file-level names are chosen before the
    // helpers inside them), ties broken by source position.
    let mut order: Vec<usize> = (0..model.bindings.len())
        .filter(|&id| model.bindings[id].renamable)
        .collect();
    order.sort_by_key(|&id| {
        let binding = &model.bindings[id];
        let span = model.scopes[binding.scope].span;
        (std::cmp::Reverse(span.end - span.start), binding.decl.start)
    });

    let total = order.len();
    observer.identifiers_found(total);

    for (index, binding_id) in order.into_iter().enumerate() {
        let current = index + 1;
        let original_name = model.bindings[binding_id].name.clone();
        observer.rename_started(current, total, &original_name);

        let surrounding = {
            let binding = &model.bindings[binding_id];
            let scope_span = model.scopes[binding.scope].span;
            let is_root = binding.scope == model.root();
            compute_context_window(source, binding.decl, scope_span, is_root, context_size)
        };

        let new_name = renamer.rename(&original_name, &surrounding);

        if new_name == original_name {
            // No rename; short-circuit — skip safe-name pipeline.
            observer.rename_finished(current, total, &original_name, &original_name);
            continue;
        }

        // Apply safe-name normalization to get the base candidate.
        let base = super::safe_name::to_identifier(&new_name);

        // Resolve collisions in this scope chain with a numeric suffix. If the
        // model's name already ends in a number, that number is incremented
        // rather than a second one appended — so `...Iterator2` ->
        // `...Iterator3`, never `...Iterator22`.
        let (safe, _collided) =
            collision::dedupe(&base, |cand| collision::collides(&model, binding_id, cand));

        // Record the new name so later collision checks see it.
        model.bindings[binding_id].name = safe.clone();
        observer.rename_finished(current, total, &original_name, &safe);
    }

    Ok(emit(source, &model))
}

/// Rewrite every reference of every renamed binding in the original source.
/// Because we splice by byte span, formatting and comments are untouched.
fn emit(source: &str, model: &ScopeModel) -> String {
    let mut edits: Vec<(Span, &str)> = Vec::new();
    for binding in &model.bindings {
        if !binding.renamable {
            continue;
        }
        let original = &source[binding.decl.start..binding.decl.end];
        if original == binding.name {
            continue;
        }
        for &span in &binding.refs {
            edits.push((span, &binding.name));
        }
    }
    edits.sort_by_key(|(span, _)| span.start);

    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for (span, name) in edits {
        out.push_str(&source[cursor..span.start]);
        out.push_str(name);
        cursor = span.end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Compute the context window slice from source, given the binding's scope span
/// and the declaring token's span.
///
/// Small scopes are sent whole. For larger ones: a chunk-level (root) binding
/// gets a window centred on the declaration; an inner scope gets its first
/// `context_size` bytes (which start at the function/loop header, the most
/// informative part).
fn compute_context_window(
    source: &str,
    decl: Span,
    ctx_span: Span,
    is_root: bool,
    context_size: usize,
) -> String {
    let ctx_start = floor_char_boundary(source, ctx_span.start);
    let ctx_end = ceil_char_boundary(source, ctx_span.end);
    let ctx_len = ctx_end.saturating_sub(ctx_start);

    if ctx_len <= context_size {
        return source[ctx_start..ctx_end].to_string();
    }

    if is_root {
        let sym_start = decl.start.min(source.len());
        let sym_end = decl.end.min(source.len());
        let half = context_size / 2;
        if sym_end < half {
            let end = ceil_char_boundary(source, context_size);
            return source[..end].to_string();
        }
        if sym_start > source.len().saturating_sub(half) {
            let start = floor_char_boundary(source, source.len().saturating_sub(context_size));
            return source[start..].to_string();
        }
        let start = floor_char_boundary(source, sym_start.saturating_sub(half));
        let end = ceil_char_boundary(source, sym_end + half);
        source[start..end].to_string()
    } else {
        let end = ceil_char_boundary(source, ctx_start + context_size).min(ctx_end);
        source[ctx_start..end].to_string()
    }
}

/// Round `index` down to the nearest UTF-8 char boundary. Clamps to `source.len()`.
fn floor_char_boundary(source: &str, index: usize) -> usize {
    let mut idx = index.min(source.len());
    while !source.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Round `index` up to the nearest UTF-8 char boundary. Clamps to `source.len()`.
fn ceil_char_boundary(source: &str, index: usize) -> usize {
    let len = source.len();
    let mut idx = index.min(len);
    while idx < len && !source.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::super::test_dsl::{
        fixed, identity, mapping, queue, recording, scenario, suffix, IdentityRenamer,
    };
    use super::*;

    #[test]
    fn no_op_returns_same_code() {
        scenario("local a = 1")
            .renamed_with(identity())
            .yields("local a = 1");
    }

    #[test]
    fn no_op_returns_same_empty_code() {
        let out = rename_all_identifiers("", &mut IdentityRenamer, 500).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn renames_simple_variable() {
        scenario("local a = 1")
            .renamed_with(fixed("b"))
            .yields("local b = 1");
    }

    #[test]
    fn renames_all_references() {
        scenario("local a = 1\nprint(a, a + a)")
            .renamed_with(fixed("count"))
            .yields("local count = 1\nprint(count, count + count)");
    }

    #[test]
    fn renames_upvalue_inside_closure() {
        scenario("local a = 1\nlocal function f() a = 2 end")
            .with_context_size(500)
            .renamed_with(mapping(&[("a", "b")]))
            .yields("local b = 1\nlocal function f() b = 2 end");
    }

    #[test]
    fn renames_two_scopes_largest_first() {
        scenario("local a = 1\nlocal function f() local b = 2 end")
            .with_context_size(500)
            .renamed_with(queue(&["c", "g", "d"]))
            .yields("local c = 1\nlocal function g() local d = 2 end");
    }

    #[test]
    fn renames_shadowed_variables_independently() {
        // Two distinct bindings named `a` — each gets its own rename call.
        scenario("local a = 1\nlocal function f() local a = 2 return a end")
            .with_context_size(500)
            .renamed_with(queue(&["b", "f", "c"]))
            .yields("local b = 1\nlocal function f() local c = 2 return c end");
    }

    #[test]
    fn renames_parameters() {
        scenario("local function f(a, b) return a + b end")
            .renamed_with(mapping(&[("a", "left"), ("b", "right")]))
            .yields("local function f(left, right) return left + right end");
    }

    #[test]
    fn renames_loop_variables() {
        scenario("for i = 1, 3 do for k, v in pairs(t) do print(i, k, v) end end")
            .renamed_with(mapping(&[("i", "index"), ("k", "key"), ("v", "value")]))
            .yields(
                "for index = 1, 3 do for key, value in pairs(t) do print(index, key, value) end end",
            );
    }

    #[test]
    fn does_not_rename_globals() {
        scenario("function foo() return bar end\nprint(foo())")
            .renamed_with(suffix("_changed"))
            .yields("function foo() return bar end\nprint(foo())");
    }

    #[test]
    fn does_not_rename_fields_or_methods() {
        scenario("local t = { foo = 1 }\nt.foo = 2\nt:bar()\nprint(t.foo)")
            .renamed_with(suffix("_changed"))
            .yields(
                "local t_changed = { foo = 1 }\nt_changed.foo = 2\nt_changed:bar()\nprint(t_changed.foo)",
            );
    }

    #[test]
    fn does_not_rename_implicit_self() {
        scenario("local T = {}\nfunction T:m(a) return self, a end")
            .renamed_with(suffix("_x"))
            .yields("local T_x = {}\nfunction T_x:m(a_x) return self, a_x end");
    }

    #[test]
    fn renames_local_function_recursively() {
        scenario("local function f(n) if n == 0 then return 1 end return n * f(n - 1) end")
            .renamed_with(mapping(&[("f", "factorial"), ("n", "count")]))
            .yields(
                "local function factorial(count) if count == 0 then return 1 end return count * factorial(count - 1) end",
            );
    }

    #[test]
    fn preserves_comments_and_whitespace() {
        scenario("-- header\nlocal  a   = 1 --[[ trailing ]]\n\n\nprint( a )\n")
            .renamed_with(fixed("b"))
            .yields("-- header\nlocal  b   = 1 --[[ trailing ]]\n\n\nprint( b )");
    }

    #[test]
    fn normalizes_unsafe_suggestions() {
        scenario("local a = 1")
            .renamed_with(fixed("end"))
            .yields("local _end = 1");
    }

    #[test]
    fn deduplicates_same_scope_collision() {
        scenario("local a, b = 1, 2")
            .renamed_with(fixed("value"))
            .yields("local value, value2 = 1, 2");
    }

    #[test]
    fn deduplicates_against_global_used_in_scope() {
        scenario("local a = 1\nprint(a)")
            .renamed_with(fixed("print"))
            .yields("local print2 = 1\nprint(print2)");
    }

    #[test]
    fn does_not_capture_outer_upvalue() {
        scenario("local x = 1\nlocal function f() local a = 2 return a + x end")
            .renamed_with(mapping(&[("a", "x")]))
            .yields("local x = 1\nlocal function f() local x2 = 2 return x2 + x end");
    }

    #[test]
    fn outer_rename_avoids_inner_binding() {
        scenario("local a = 1\nlocal function f() local x = 2 return a end")
            .renamed_with(mapping(&[("a", "x")]))
            .yields("local x2 = 1\nlocal function f() local x = 2 return x2 end");
    }

    #[test]
    fn sibling_scopes_may_share_names() {
        scenario("local function f() local a = 1 return a end\nlocal function g() local b = 2 return b end")
            .renamed_with(mapping(&[("a", "value"), ("b", "value")]))
            .yields("local function f() local value = 1 return value end\nlocal function g() local value = 2 return value end");
    }

    #[test]
    fn increments_existing_numeric_suffix() {
        scenario("local a, b = 1, 2")
            .renamed_with(fixed("item3"))
            .yields("local item3, item4 = 1, 2");
    }

    #[test]
    fn parse_error_is_reported() {
        let err = rename_all_identifiers("local = ", &mut IdentityRenamer, 500).unwrap_err();
        let RenameError::Parse(msg) = err;
        assert!(!msg.is_empty());
    }

    #[test]
    fn accepts_lua54_syntax() {
        scenario("local x <const> = 1\nlocal y = x // 2")
            .renamed_with(mapping(&[("x", "limit")]))
            .yields("local limit <const> = 1\nlocal y = limit // 2");
    }

    #[test]
    fn accepts_goto_and_labels() {
        scenario("local n = 0\n::top::\nn = n + 1\nif n < 3 then goto top end")
            .renamed_with(fixed("counter"))
            .yields("local counter = 0\n::top::\ncounter = counter + 1\nif counter < 3 then goto top end");
    }

    #[test]
    fn source_round_trips_when_identity() {
        scenario(
            "local M = {}\n\nfunction M.new(x, y)\n  local self = setmetatable({}, M)\n  self.x, self.y = x, y\n  return self\nend\n\nreturn M\n",
        )
        .parses_unchanged();
    }

    const SCOPE_INPUT: &str = "local a = 1\nlocal function foo()\n  local b = 2\n  local function bar()\n    local y = 123\n  end\nend\n";

    #[test]
    fn passes_surrounding_scope_argument() {
        let (_, log) = scenario(SCOPE_INPUT)
            .with_context_size(500)
            .with_recording(recording("_changed"));
        assert_eq!(
            log.0.len(),
            5,
            "expected 5 calls, got {}: {:?}",
            log.0.len(),
            log.0
        );
    }

    #[test]
    fn passes_identifiers_in_scope_order() {
        let (_, log) = scenario(SCOPE_INPUT)
            .with_context_size(500)
            .with_recording(recording("_x"));
        assert_eq!(log.call_names(), &["a", "foo", "b", "bar", "y"]);
    }

    #[test]
    fn scope_for_inner_binding_is_enclosing_function() {
        let (_, log) = scenario(SCOPE_INPUT)
            .with_context_size(500)
            .with_recording(recording("_x"));
        let scope = log.scope_for("b");
        assert!(scope.contains("local b = 2"), "scope: {scope}");
        assert!(scope.starts_with("()"), "scope: {scope}");
        assert!(!scope.contains("local a = 1"), "scope: {scope}");
    }

    #[test]
    fn scope_for_root_binding_is_whole_file() {
        let (_, log) = scenario(SCOPE_INPUT)
            .with_context_size(500)
            .with_recording(recording("_x"));
        assert_eq!(log.scope_for("a"), SCOPE_INPUT);
    }

    #[test]
    fn scopes_renamed_largest_to_smallest() {
        let input =
            "local function foo() local function bar() local function baz() local qux = 1 end end end";
        let (_, log) = scenario(input)
            .with_context_size(500)
            .with_recording(recording("_x"));
        assert_eq!(log.call_names(), &["foo", "bar", "baz", "qux"]);
    }

    #[test]
    fn context_truncated_for_inner_scope() {
        let input = "local function f(a)\n  local b = a\n  return b\nend\n";
        let (_, log) = scenario(input)
            .with_context_size(10)
            .with_recording(recording("_x"));
        assert_eq!(log.scope_for("a"), "(a)\n  loca");
    }

    #[test]
    fn context_for_root_binding_is_centred_on_declaration() {
        let input = "local a = 1\nlocal b = 2\nlocal c = 3\nlocal d = 4\nlocal e = 5\n";
        let (_, log) = scenario(input)
            .with_context_size(12)
            .with_recording(recording("_x"));
        // `a` sits near the start: window is [decl-6, decl+1+6).
        assert_eq!(log.scope_for("a"), "local a = 1\nl");
        // `e` sits at the end: window is clamped to the last 12 bytes.
        assert_eq!(log.scope_for("e"), "local e = 5\n");
        let mid = log.scope_for("c");
        assert!(mid.contains("local c"), "mid: {mid}");
        assert_eq!(mid.len(), 13, "mid: {mid:?}");
    }

    #[test]
    fn context_window_respects_utf8_boundaries() {
        let input = "local a = \"ééééééééééééééé\"\nlocal b = 2\n";
        let (_, log) = scenario(input)
            .with_context_size(15)
            .with_recording(recording("_x"));
        // Must not panic slicing inside a multi-byte char.
        assert!(!log.scope_for("a").is_empty());
    }

    #[test]
    fn floor_and_ceil_char_boundary_clamp() {
        assert_eq!(floor_char_boundary("abc", 10), 3);
        assert_eq!(ceil_char_boundary("abc", 10), 3);
        assert_eq!(floor_char_boundary("é", 1), 0);
        assert_eq!(ceil_char_boundary("é", 1), 2);
    }

    #[test]
    fn varargs_are_ignored_not_renamed() {
        scenario("local function f(...) local t = {...} return t end")
            .with_context_size(500)
            .renamed_with(mapping(&[("f", "collect"), ("t", "items")]))
            .yields("local function collect(...) local items = {...} return items end");
    }

    #[test]
    fn vararg_forwarding_call() {
        scenario("local function f(...) return print(...) end")
            .renamed_with(fixed("wrapper"))
            .yields("local function wrapper(...) return print(...) end");
    }

    #[test]
    fn iife_anonymous_function_call() {
        scenario("local x = (function() local y = 1 return y end)()")
            .with_context_size(500)
            .renamed_with(mapping(&[("x", "result"), ("y", "one")]))
            .yields("local result = (function() local one = 1 return one end)()");
    }

    #[test]
    fn lua54_close_attribute() {
        scenario("local x <close> = acquire()")
            .renamed_with(fixed("resource"))
            .yields("local resource <close> = acquire()");
    }

    #[test]
    fn bracket_index_assignment_array_push_idiom() {
        scenario("local n = {} n[#n+1] = 1")
            .with_context_size(500)
            .renamed_with(fixed("items"))
            .yields("local items = {} items[#items+1] = 1");
    }

    #[test]
    fn deep_dotted_function_declaration_only_renames_base() {
        scenario("local a = {} a.b = {} function a.b.c() end")
            .with_context_size(500)
            .renamed_with(fixed("root"))
            .yields("local root = {} root.b = {} function root.b.c() end");
    }

    // --- CFX Lua (FiveM) extensions ---

    #[test]
    fn cfx_compound_assignment_renames_variable() {
        scenario("local x = 1 x += 1")
            .renamed_with(fixed("counter"))
            .yields("local counter = 1 counter += 1");
    }

    #[test]
    fn cfx_compound_assignment_bitwise_operators() {
        scenario("local x = 0 x &= 1 x |= 2 x <<= 3 x >>= 4")
            .renamed_with(fixed("flags"))
            .yields("local flags = 0 flags &= 1 flags |= 2 flags <<= 3 flags >>= 4");
    }

    #[test]
    fn cfx_compound_assignment_on_indexed_variable() {
        scenario("local t = {x = 1} t.x += 1")
            .renamed_with(fixed("state"))
            .yields("local state = {x = 1} state.x += 1");
    }

    #[test]
    fn cfx_safe_navigation_field_access_not_renamed() {
        scenario("local t = {} local r = t?.a?.b")
            .with_context_size(500)
            .renamed_with(mapping(&[("t", "obj"), ("r", "result")]))
            .yields("local obj = {} local result = obj?.a?.b");
    }

    #[test]
    fn cfx_set_constructor_shorthand_field_not_a_variable() {
        // `{ .a }` means `{ a = true }` — `a` is a field name, not a reference
        // to the local variable also named `a`, so renaming that local must
        // leave `.a` untouched.
        scenario("local a = 1 local t = { .a }")
            .with_context_size(500)
            .renamed_with(mapping(&[("a", "value")]))
            .yields("local value = 1 local t = { .a }");
    }

    #[test]
    fn cfx_local_in_unpacking_sugar() {
        // `local a, b in t` unpacks named fields (`t.a`, `t.b`) rather than
        // assigning `t` itself; structurally it's still a `LocalAssignment`
        // with `t` as the sole expression, which our walker already handles
        // generically — declare the new locals, reference `t` once.
        scenario("local player = get_player() local id, name in player print(id, name)")
            .with_context_size(500)
            .renamed_with(mapping(&[("player", "src"), ("id", "player_id"), ("name", "player_name")]))
            .yields(
                "local src = get_player() local player_id, player_name in src print(player_id, player_name)",
            );
    }

    #[test]
    fn cfx_backtick_jenkins_hash_string_untouched() {
        // Backtick-quoted strings are Jenkins-hashed at parse time by CFX Lua;
        // they're a string literal to us, not an identifier — must survive
        // renaming byte-for-byte.
        scenario("local h = `some_event_name`")
            .renamed_with(fixed("hash"))
            .yields("local hash = `some_event_name`");
    }

    #[test]
    fn cfx_c_style_block_comment_preserved() {
        scenario("local a = 1 /* keep this comment */ local b = 2")
            .with_context_size(500)
            .renamed_with(mapping(&[("a", "first"), ("b", "second")]))
            .yields("local first = 1 /* keep this comment */ local second = 2");
    }
}

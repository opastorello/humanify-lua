//! Lexical scope model for a Lua chunk.
//!
//! `full_moon` gives us a lossless AST but no semantic analysis, so this module
//! walks the tree once, in source order, and records:
//!
//! * every scope (chunk, function body, `do`, loop bodies, `if` branches,
//!   `repeat ... until`) with its parent and byte range;
//! * every **local** binding (`local x`, `local function f`, parameters,
//!   `for` variables, the implicit `self` of `function t:m()`), the scope it
//!   lives in and every identifier token that resolves to it;
//! * every identifier that resolves to nothing — a global.
//!
//! Only local bindings are ever renamed: a global may be provided by the host
//! or by another chunk, so renaming it would change program behaviour.
//!
//! Resolution is done during the walk. Because declarations are pushed onto
//! their scope exactly when Lua makes them visible (after a `local` statement's
//! expressions, before a `local function`'s body, at the top of a loop/function
//! body), a lookup at the moment a reference is visited yields the right
//! binding — including `local x = x` and same-scope shadowing.

use full_moon::ast::{
    Ast, Block, Call, Expression, Field, FunctionArgs, FunctionBody, Index, LastStmt, Parameter,
    Prefix, Stmt, Suffix, Var,
};
use full_moon::node::Node;
use full_moon::tokenizer::{Token, TokenReference, TokenType};

pub(crate) type ScopeId = usize;
pub(crate) type BindingId = usize;

/// Half-open byte range `[start, end)` into the original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub(crate) fn contains(&self, pos: usize) -> bool {
        self.start <= pos && pos < self.end
    }
}

#[derive(Debug)]
pub(crate) struct Scope {
    pub parent: Option<ScopeId>,
    pub span: Span,
    /// Bindings declared directly in this scope, in declaration order. Later
    /// entries shadow earlier ones with the same name.
    pub bindings: Vec<BindingId>,
}

#[derive(Debug)]
pub(crate) struct Binding {
    /// Current name. Updated by the walker when the binding is renamed so that
    /// later collision checks see the new name.
    pub name: String,
    pub scope: ScopeId,
    /// Span of the declaring identifier token.
    pub decl: Span,
    /// Every identifier token that resolves to this binding, including `decl`.
    /// Empty for implicit bindings (`self`).
    pub refs: Vec<Span>,
    /// `false` for bindings that have no token to rewrite (`self`).
    pub renamable: bool,
}

/// An identifier that did not resolve to any local: a global.
#[derive(Debug)]
pub(crate) struct FreeRef {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Default)]
pub(crate) struct ScopeModel {
    pub scopes: Vec<Scope>,
    pub bindings: Vec<Binding>,
    pub free_refs: Vec<FreeRef>,
}

impl ScopeModel {
    pub(crate) fn build(ast: &Ast, source_len: usize) -> Self {
        let mut builder = Builder {
            model: ScopeModel::default(),
            stack: Vec::new(),
        };
        let root = builder.push_scope(Span {
            start: 0,
            end: source_len,
        });
        builder.walk_block(ast.nodes());
        builder.pop_scope(root);
        builder.model
    }

    pub(crate) fn root(&self) -> ScopeId {
        0
    }

    /// True if `scope` is `ancestor` itself or nested (at any depth) inside it.
    pub(crate) fn is_within(&self, mut scope: ScopeId, ancestor: ScopeId) -> bool {
        loop {
            if scope == ancestor {
                return true;
            }
            match self.scopes[scope].parent {
                Some(p) => scope = p,
                None => return false,
            }
        }
    }
}

struct Builder {
    model: ScopeModel,
    stack: Vec<ScopeId>,
}

// `TokenReference` implements both the inherent (non-Option) `Token` position
// methods (via `Deref`) and the `Node` trait's (`Option`-returning) ones of the
// same name; plain method-call syntax resolves to the `Node` trait version
// before deref kicks in. These UFCS-based helpers force the `Token` version,
// which is infallible for a real token.
fn tok_start(token: &TokenReference) -> usize {
    Token::start_position(token).bytes()
}

fn tok_end(token: &TokenReference) -> usize {
    Token::end_position(token).bytes()
}

fn token_span(token: &TokenReference) -> Span {
    Span {
        start: tok_start(token),
        end: tok_end(token),
    }
}

fn identifier_name(token: &TokenReference) -> Option<&str> {
    match token.token_type() {
        TokenType::Identifier { identifier } => Some(identifier.as_str()),
        _ => None,
    }
}

/// Byte range of a node, or the zero-width span at `fallback` when the node
/// has no tokens of its own (an empty block).
fn node_span<N: Node>(node: &N, fallback: usize) -> Span {
    match node.range() {
        Some((start, end)) => Span {
            start: start.bytes(),
            end: end.bytes(),
        },
        None => Span {
            start: fallback,
            end: fallback,
        },
    }
}

impl Builder {
    fn current(&self) -> ScopeId {
        *self
            .stack
            .last()
            .expect("scope stack is never empty during walk")
    }

    fn push_scope(&mut self, span: Span) -> ScopeId {
        let parent = self.stack.last().copied();
        let id = self.model.scopes.len();
        self.model.scopes.push(Scope {
            parent,
            span,
            bindings: Vec::new(),
        });
        self.stack.push(id);
        id
    }

    fn pop_scope(&mut self, id: ScopeId) {
        let popped = self.stack.pop();
        debug_assert_eq!(popped, Some(id));
    }

    fn declare(&mut self, token: &TokenReference) {
        let Some(name) = identifier_name(token) else {
            return;
        };
        let span = token_span(token);
        let scope = self.current();
        let id = self.model.bindings.len();
        self.model.bindings.push(Binding {
            name: name.to_string(),
            scope,
            decl: span,
            refs: vec![span],
            renamable: true,
        });
        self.model.scopes[scope].bindings.push(id);
    }

    fn declare_implicit(&mut self, name: &str, at: usize) {
        let scope = self.current();
        let id = self.model.bindings.len();
        self.model.bindings.push(Binding {
            name: name.to_string(),
            scope,
            decl: Span { start: at, end: at },
            refs: Vec::new(),
            renamable: false,
        });
        self.model.scopes[scope].bindings.push(id);
    }

    fn reference(&mut self, token: &TokenReference) {
        let Some(name) = identifier_name(token) else {
            return;
        };
        let span = token_span(token);
        let mut scope = Some(self.current());
        while let Some(s) = scope {
            if let Some(&binding) = self.model.scopes[s]
                .bindings
                .iter()
                .rev()
                .find(|&&b| self.model.bindings[b].name == name)
            {
                self.model.bindings[binding].refs.push(span);
                return;
            }
            scope = self.model.scopes[s].parent;
        }
        self.model.free_refs.push(FreeRef {
            name: name.to_string(),
            span,
        });
    }

    fn walk_block(&mut self, block: &Block) {
        for stmt in block.stmts() {
            self.walk_stmt(stmt);
        }
        if let Some(LastStmt::Return(ret)) = block.last_stmt() {
            for expr in ret.returns() {
                self.walk_expr(expr);
            }
        }
    }

    /// A block that introduces its own scope (`do`, `while`, `if` branches).
    fn walk_scoped_block(&mut self, block: &Block, fallback: usize) {
        let id = self.push_scope(node_span(block, fallback));
        self.walk_block(block);
        self.pop_scope(id);
    }

    fn walk_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assignment(assignment) => {
                // Lua evaluates the right-hand side before assigning, but both
                // sides resolve against the same environment.
                for expr in assignment.expressions() {
                    self.walk_expr(expr);
                }
                for var in assignment.variables() {
                    self.walk_var(var);
                }
            }
            // CFX Lua / Luau compound assignment, e.g. `x += 1`. `x` is both
            // read and written, same as a plain `Assignment`'s variable side.
            Stmt::CompoundAssignment(compound) => {
                self.walk_expr(compound.rhs());
                self.walk_var(compound.lhs());
            }
            Stmt::Do(do_stmt) => {
                self.walk_scoped_block(do_stmt.block(), tok_end(do_stmt.do_token()));
            }
            Stmt::FunctionCall(call) => {
                self.walk_prefix(call.prefix());
                for suffix in call.suffixes() {
                    self.walk_suffix(suffix);
                }
            }
            Stmt::FunctionDeclaration(decl) => {
                // `function a.b.c:m()` assigns into `a`; only the first name is
                // a variable reference, the rest are table fields.
                if let Some(first) = decl.name().names().iter().next() {
                    self.reference(first);
                }
                let is_method = decl.name().method_colon().is_some();
                self.walk_function_body(decl.body(), is_method);
            }
            Stmt::GenericFor(generic_for) => {
                for expr in generic_for.expressions() {
                    self.walk_expr(expr);
                }
                let id =
                    self.push_scope(node_span(generic_for, tok_start(generic_for.for_token())));
                for name in generic_for.names() {
                    self.declare(name);
                }
                self.walk_block(generic_for.block());
                self.pop_scope(id);
            }
            Stmt::If(if_stmt) => {
                self.walk_expr(if_stmt.condition());
                self.walk_scoped_block(if_stmt.block(), tok_end(if_stmt.then_token()));
                if let Some(else_ifs) = if_stmt.else_if() {
                    for else_if in else_ifs {
                        self.walk_expr(else_if.condition());
                        self.walk_scoped_block(else_if.block(), tok_end(else_if.then_token()));
                    }
                }
                if let Some(else_block) = if_stmt.else_block() {
                    let fallback = if_stmt
                        .else_token()
                        .map(tok_end)
                        .unwrap_or_else(|| tok_start(if_stmt.end_token()));
                    self.walk_scoped_block(else_block, fallback);
                }
            }
            Stmt::LocalAssignment(local) => {
                // `local x = x`: the initialiser sees the *outer* x.
                for expr in local.expressions() {
                    self.walk_expr(expr);
                }
                for name in local.names() {
                    self.declare(name);
                }
            }
            Stmt::LocalFunction(local_fn) => {
                // Visible inside its own body (recursion), unlike
                // `local f = function() ... end`.
                self.declare(local_fn.name());
                self.walk_function_body(local_fn.body(), false);
            }
            Stmt::NumericFor(numeric_for) => {
                self.walk_expr(numeric_for.start());
                self.walk_expr(numeric_for.end());
                if let Some(step) = numeric_for.step() {
                    self.walk_expr(step);
                }
                let id =
                    self.push_scope(node_span(numeric_for, tok_start(numeric_for.for_token())));
                self.declare(numeric_for.index_variable());
                self.walk_block(numeric_for.block());
                self.pop_scope(id);
            }
            Stmt::Repeat(repeat) => {
                // The `until` condition can see locals declared in the body.
                let id = self.push_scope(node_span(repeat, tok_start(repeat.repeat_token())));
                self.walk_block(repeat.block());
                self.walk_expr(repeat.until());
                self.pop_scope(id);
            }
            Stmt::While(while_stmt) => {
                self.walk_expr(while_stmt.condition());
                self.walk_scoped_block(while_stmt.block(), tok_end(while_stmt.do_token()));
            }
            // `goto` / labels name jump targets, not variables.
            _ => {}
        }
    }

    fn walk_function_body(&mut self, body: &FunctionBody, is_method: bool) {
        let id = self.push_scope(node_span(
            body,
            tok_start(body.parameters_parentheses().tokens().0),
        ));
        if is_method {
            let at = tok_end(body.parameters_parentheses().tokens().0);
            self.declare_implicit("self", at);
        }
        for param in body.parameters() {
            if let Parameter::Name(name) = param {
                self.declare(name);
            }
        }
        self.walk_block(body.block());
        self.pop_scope(id);
    }

    fn walk_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::BinaryOperator { lhs, rhs, .. } => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            Expression::Parentheses { expression, .. }
            | Expression::UnaryOperator { expression, .. } => {
                self.walk_expr(expression);
            }
            Expression::Function(func) => {
                self.walk_function_body(func.body(), false);
            }
            Expression::FunctionCall(call) => {
                self.walk_prefix(call.prefix());
                for suffix in call.suffixes() {
                    self.walk_suffix(suffix);
                }
            }
            Expression::TableConstructor(table) => {
                for field in table.fields() {
                    self.walk_field(field);
                }
            }
            Expression::Var(var) => self.walk_var(var),
            // Number / String / Symbol literals reference nothing.
            _ => {}
        }
    }

    fn walk_field(&mut self, field: &Field) {
        match field {
            Field::ExpressionKey { key, value, .. } => {
                self.walk_expr(key);
                self.walk_expr(value);
            }
            // `{ name = value }`: `name` is a string key, not a variable.
            Field::NameKey { value, .. } => self.walk_expr(value),
            Field::NoKey(expr) => self.walk_expr(expr),
            _ => {}
        }
    }

    fn walk_var(&mut self, var: &Var) {
        match var {
            Var::Name(name) => self.reference(name),
            Var::Expression(var_expr) => {
                self.walk_prefix(var_expr.prefix());
                for suffix in var_expr.suffixes() {
                    self.walk_suffix(suffix);
                }
            }
            _ => {}
        }
    }

    fn walk_prefix(&mut self, prefix: &Prefix) {
        match prefix {
            Prefix::Name(name) => self.reference(name),
            Prefix::Expression(expr) => self.walk_expr(expr),
            _ => {}
        }
    }

    fn walk_suffix(&mut self, suffix: &Suffix) {
        match suffix {
            Suffix::Call(Call::AnonymousCall(args)) => self.walk_args(args),
            Suffix::Call(Call::MethodCall(method)) => self.walk_args(method.args()),
            Suffix::Index(Index::Brackets { expression, .. }) => self.walk_expr(expression),
            // `.name` is a field, not a variable.
            Suffix::Index(Index::Dot { .. }) => {}
            _ => {}
        }
    }

    fn walk_args(&mut self, args: &FunctionArgs) {
        match args {
            FunctionArgs::Parentheses { arguments, .. } => {
                for expr in arguments {
                    self.walk_expr(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                for field in table.fields() {
                    self.walk_field(field);
                }
            }
            // `f "str"` passes a literal.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_dsl::model_of;

    #[test]
    fn local_declares_binding_in_root() {
        model_of("local a = 1").binding("a").in_root().has_refs(1);
    }

    #[test]
    fn references_resolve_to_local() {
        model_of("local a = 1\nprint(a + a)")
            .binding("a")
            .has_refs(3);
    }

    #[test]
    fn unknown_names_are_free() {
        model_of("print(a)").free("a").free("print");
    }

    #[test]
    fn local_initialiser_sees_outer_binding() {
        // The `x` on the right-hand side is the *first* x.
        let m = model_of("local x = 1\nlocal x = x + 1");
        m.nth_binding("x", 0).has_refs(2);
        m.nth_binding("x", 1).has_refs(1);
    }

    #[test]
    fn same_scope_shadowing_resolves_to_latest() {
        let m = model_of("local x = 1\nlocal x = 2\nprint(x)");
        m.nth_binding("x", 0).has_refs(1);
        m.nth_binding("x", 1).has_refs(2);
    }

    #[test]
    fn local_function_is_visible_in_own_body() {
        model_of("local function f() return f() end")
            .binding("f")
            .has_refs(2);
    }

    #[test]
    fn local_anonymous_function_is_not_visible_in_own_body() {
        let m = model_of("local f = function() return f() end");
        m.binding("f").has_refs(1);
        m.free("f");
    }

    #[test]
    fn parameters_are_bindings_in_function_scope() {
        let m = model_of("local function f(a, b) return a + b end");
        m.binding("a").not_in_root().has_refs(2);
        m.binding("b").not_in_root().has_refs(2);
    }

    #[test]
    fn method_declares_implicit_self() {
        // `self` has no declaring token (nothing to rename), but the `self.x`
        // usage still resolves to it rather than showing up as a free
        // reference — needed so collision detection knows `self` is taken.
        let m = model_of("function T:m() return self.x end");
        m.binding("self").is_not_renamable().has_refs(1);
        m.free("T");
    }

    #[test]
    fn global_function_name_is_free_reference() {
        model_of("function foo() end").free("foo");
    }

    #[test]
    fn global_function_assigns_into_local_table() {
        model_of("local t = {}\nfunction t.foo() end\nfunction t:bar() end")
            .binding("t")
            .has_refs(3);
    }

    #[test]
    fn dot_fields_and_method_names_are_not_references() {
        let m = model_of("local t = {}\nt.foo = 1\nt:bar()\nlocal k = { foo = 2 }");
        m.no_binding("foo").no_free("foo").no_free("bar");
    }

    #[test]
    fn bracket_index_is_an_expression() {
        model_of("local t, k = {}, 1\nprint(t[k])")
            .binding("k")
            .has_refs(2);
    }

    #[test]
    fn numeric_for_variable_scoped_to_loop() {
        let m = model_of("for i = 1, 10 do print(i) end\nprint(i)");
        m.binding("i").not_in_root().has_refs(2);
        m.free("i");
    }

    #[test]
    fn generic_for_variables_scoped_to_loop() {
        let m = model_of("for k, v in pairs(t) do print(k, v) end");
        m.binding("k").not_in_root().has_refs(2);
        m.binding("v").not_in_root().has_refs(2);
        m.free("t").free("pairs");
    }

    #[test]
    fn numeric_for_bounds_evaluated_outside_loop_scope() {
        // `for i = i, ...` — the bound `i` is the outer one.
        let m = model_of("local i = 1\nfor i = i, 3 do end");
        m.nth_binding("i", 0).has_refs(2);
        m.nth_binding("i", 1).has_refs(1);
    }

    #[test]
    fn repeat_until_sees_body_locals() {
        model_of("repeat local done = true until done")
            .binding("done")
            .has_refs(2);
    }

    #[test]
    fn do_block_introduces_scope() {
        let m = model_of("do local a = 1 end\nprint(a)");
        m.binding("a").not_in_root().has_refs(1);
        m.free("a");
    }

    #[test]
    fn if_branches_have_separate_scopes() {
        let m = model_of("if x then local a = 1 elseif y then local a = 2 else local a = 3 end");
        assert_eq!(m.bindings_named("a"), 3);
        m.free("x").free("y");
    }

    #[test]
    fn while_condition_outside_body_scope() {
        let m = model_of("while n do local n = 1 end");
        m.binding("n").has_refs(1);
        m.free("n");
    }

    #[test]
    fn closures_capture_upvalues() {
        model_of("local n = 0\nlocal function inc() n = n + 1 end")
            .binding("n")
            .has_refs(3);
    }

    #[test]
    fn scope_spans_nest() {
        let m = model_of("local function f()\n  local a = 1\nend");
        let root = m.scope_of("f");
        let inner = m.scope_of("a");
        assert_ne!(root, inner);
        assert!(m.model().is_within(inner, root));
        assert!(!m.model().is_within(root, inner));
        let span = m.model().scopes[inner].span;
        assert!(span.start > 0 && span.end <= m.source().len());
    }
}

use std::collections::VecDeque;

use super::scope::{BindingId, ScopeId, ScopeModel};
use super::{rename_all_identifiers, Renamer};

// --- Renamer constructors ---

pub struct FixedRenamer(String);
impl Renamer for FixedRenamer {
    fn rename(&mut self, _: &str, _: &str) -> String {
        self.0.clone()
    }
}

pub struct QueueRenamer(VecDeque<String>);
impl Renamer for QueueRenamer {
    fn rename(&mut self, original: &str, _: &str) -> String {
        self.0.pop_front().unwrap_or_else(|| original.to_string())
    }
}

pub struct SuffixRenamer(String);
impl Renamer for SuffixRenamer {
    fn rename(&mut self, original: &str, _: &str) -> String {
        format!("{original}{}", self.0)
    }
}

pub struct IdentityRenamer;
impl Renamer for IdentityRenamer {
    fn rename(&mut self, original: &str, _: &str) -> String {
        original.to_string()
    }
}

/// Renames based on the original name via a lookup table; unmapped names are
/// left unchanged. Order-independent, unlike `queue`, which makes it convenient
/// for testing scope-aware collision behaviour regardless of traversal order.
pub struct MapRenamer(std::collections::HashMap<String, String>);
impl Renamer for MapRenamer {
    fn rename(&mut self, original: &str, _: &str) -> String {
        self.0
            .get(original)
            .cloned()
            .unwrap_or_else(|| original.to_string())
    }
}

pub struct RecordingRenamer {
    suffix: String,
    pub log: CallLog,
}

/// Captures `(original, surrounding)` pairs for each rename call.
#[derive(Default, Clone)]
pub struct CallLog(pub Vec<(String, String)>);

impl CallLog {
    pub fn call_names(&self) -> Vec<&str> {
        self.0.iter().map(|(n, _)| n.as_str()).collect()
    }

    pub fn scope_for(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.as_str())
            .unwrap_or_else(|| panic!("no call recorded for '{name}'"))
    }
}

impl Renamer for RecordingRenamer {
    fn rename(&mut self, original: &str, surrounding: &str) -> String {
        self.log
            .0
            .push((original.to_string(), surrounding.to_string()));
        format!("{original}{}", self.suffix)
    }
}

pub fn fixed(name: &str) -> FixedRenamer {
    FixedRenamer(name.to_string())
}

pub fn queue(names: &[&str]) -> QueueRenamer {
    QueueRenamer(names.iter().map(|s| s.to_string()).collect())
}

pub fn suffix(sfx: &str) -> SuffixRenamer {
    SuffixRenamer(sfx.to_string())
}

pub fn identity() -> IdentityRenamer {
    IdentityRenamer
}

pub fn recording(sfx: &str) -> RecordingRenamer {
    RecordingRenamer {
        suffix: sfx.to_string(),
        log: CallLog::default(),
    }
}

pub fn mapping(pairs: &[(&str, &str)]) -> MapRenamer {
    MapRenamer(
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    )
}

// --- ScenarioBuilder ---

pub struct ScenarioBuilder {
    source: String,
    context_size: usize,
}

pub fn scenario(source: &str) -> ScenarioBuilder {
    ScenarioBuilder {
        source: source.to_string(),
        context_size: 200,
    }
}

impl ScenarioBuilder {
    pub fn with_context_size(mut self, n: usize) -> Self {
        self.context_size = n;
        self
    }

    pub fn renamed_with<R: Renamer>(self, mut renamer: R) -> RenamedScenario {
        let result = rename_all_identifiers(&self.source, &mut renamer, self.context_size);
        RenamedScenario {
            output: result.expect("rename_all_identifiers failed"),
        }
    }

    pub fn with_recording(self, mut renamer: RecordingRenamer) -> (RenamedScenario, CallLog) {
        let result = rename_all_identifiers(&self.source, &mut renamer, self.context_size);
        let log = renamer.log;
        (
            RenamedScenario {
                output: result.expect("rename_all_identifiers failed"),
            },
            log,
        )
    }

    pub fn parses_unchanged(self) {
        let output = rename_all_identifiers(&self.source, &mut IdentityRenamer, self.context_size)
            .expect("rename_all_identifiers failed");
        assert_eq!(output, self.source, "source did not round-trip");
    }
}

pub struct RenamedScenario {
    output: String,
}

impl RenamedScenario {
    pub fn yields(self, expected: &str) {
        let got = self.output.trim_end_matches('\n');
        assert_eq!(got, expected, "renamed output mismatch");
    }

    pub fn output(&self) -> &str {
        self.output.trim_end_matches('\n')
    }
}

// --- IdentifierAssertion (for safe_name tests) ---

pub struct IdentifierAssertion {
    raw: String,
    result: String,
}

pub fn to_identifier_of(raw: &str) -> IdentifierAssertion {
    IdentifierAssertion {
        raw: raw.to_string(),
        result: super::safe_name::to_identifier(raw),
    }
}

impl IdentifierAssertion {
    pub fn is(self, expected: &str) {
        assert_eq!(
            self.result, expected,
            "to_identifier({:?}) should be {expected:?}",
            self.raw
        );
    }

    pub fn is_reserved(self) {
        assert!(
            super::safe_name::is_reserved_word(&self.raw),
            "{:?} should be reserved",
            self.raw
        );
    }

    pub fn is_not_reserved(self) {
        assert!(
            !super::safe_name::is_reserved_word(&self.raw),
            "{:?} should not be reserved",
            self.raw
        );
    }
}

// --- ModelAssertion (for scope / collision tests) ---

pub struct ModelAssertion {
    source: String,
    model: ScopeModel,
}

pub fn model_of(source: &str) -> ModelAssertion {
    let parsed = full_moon::parse_fallible(source, full_moon::LuaVersion::new());
    assert!(
        parsed.errors().is_empty(),
        "fixture failed to parse: {:?}",
        parsed.errors()
    );
    ModelAssertion {
        source: source.to_string(),
        model: ScopeModel::build(parsed.ast(), source.len()),
    }
}

impl ModelAssertion {
    pub(crate) fn model(&self) -> &ScopeModel {
        &self.model
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    fn ids_named(&self, name: &str) -> Vec<BindingId> {
        (0..self.model.bindings.len())
            .filter(|&id| self.model.bindings[id].name == name)
            .collect()
    }

    pub fn bindings_named(&self, name: &str) -> usize {
        self.ids_named(name).len()
    }

    /// The single binding named `name`; panics if there are zero or several.
    pub fn binding(&self, name: &str) -> BindingAssertion<'_> {
        let ids = self.ids_named(name);
        assert_eq!(
            ids.len(),
            1,
            "expected exactly one binding named {name:?}, found {}",
            ids.len()
        );
        BindingAssertion {
            model: &self.model,
            id: ids[0],
        }
    }

    /// The `n`-th (declaration order) binding named `name`.
    pub fn nth_binding(&self, name: &str, n: usize) -> BindingAssertion<'_> {
        let ids = self.ids_named(name);
        let id = *ids
            .get(n)
            .unwrap_or_else(|| panic!("no binding #{n} named {name:?} (found {})", ids.len()));
        BindingAssertion {
            model: &self.model,
            id,
        }
    }

    pub fn scope_of(&self, name: &str) -> ScopeId {
        self.model.bindings[self.binding(name).id].scope
    }

    pub fn no_binding(&self, name: &str) -> &Self {
        assert!(
            self.ids_named(name).is_empty(),
            "expected no binding named {name:?}"
        );
        self
    }

    pub fn free(&self, name: &str) -> &Self {
        assert!(
            self.model.free_refs.iter().any(|f| f.name == name),
            "expected a free reference named {name:?}; free refs: {:?}",
            self.model.free_refs
        );
        self
    }

    pub fn no_free(&self, name: &str) -> &Self {
        assert!(
            !self.model.free_refs.iter().any(|f| f.name == name),
            "expected no free reference named {name:?}"
        );
        self
    }
}

pub struct BindingAssertion<'a> {
    model: &'a ScopeModel,
    id: BindingId,
}

impl BindingAssertion<'_> {
    pub fn has_refs(self, n: usize) -> Self {
        let refs = &self.model.bindings[self.id].refs;
        assert_eq!(
            refs.len(),
            n,
            "binding {:?} should have {n} refs, has {:?}",
            self.model.bindings[self.id].name,
            refs
        );
        self
    }

    pub fn in_root(self) -> Self {
        assert_eq!(self.model.bindings[self.id].scope, self.model.root());
        self
    }

    pub fn not_in_root(self) -> Self {
        assert_ne!(self.model.bindings[self.id].scope, self.model.root());
        self
    }

    pub fn is_not_renamable(self) -> Self {
        assert!(!self.model.bindings[self.id].renamable);
        self
    }
}

// --- Collision DSL ---

pub struct RenameAttempt {
    model: ModelAssertion,
    binding: BindingId,
    candidate: String,
}

pub fn renaming(model: ModelAssertion, name: &str) -> RenameAttemptBuilder {
    let binding = model.binding(name).id;
    RenameAttemptBuilder { model, binding }
}

pub struct RenameAttemptBuilder {
    model: ModelAssertion,
    binding: BindingId,
}

impl RenameAttemptBuilder {
    pub fn to(self, candidate: &str) -> RenameAttempt {
        RenameAttempt {
            model: self.model,
            binding: self.binding,
            candidate: candidate.to_string(),
        }
    }
}

impl RenameAttempt {
    pub fn collides(&self) -> bool {
        super::collision::collides(&self.model.model, self.binding, &self.candidate)
    }
}

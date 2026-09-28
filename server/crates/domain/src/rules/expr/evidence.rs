/// Which declaration answered, as a path from the owning rule down to the clause.
///
/// A path rather than one id, and that is the point for a reused fragment: the terminal case is the *same*
/// declaration whichever caller reached it, so the identity of the clause and the route taken to it are
/// different facts. Before this a rule with four readings reported only the rule's id, and a shape read
/// directly was indistinguishable from the same shape reached through a fragment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClausePath {
    /// The rule or fragment the answer started in.
    pub root: String,
    /// The clauses entered, outermost first. Empty means the rule itself answered.
    pub steps: Vec<String>,
}

impl ClausePath {
    /// A rule answering directly, with no inner clause.
    pub fn root(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            steps: Vec::new(),
        }
    }

    /// The same path with one more clause entered.
    pub fn then(mut self, step: impl Into<String>) -> Self {
        self.steps.push(step.into());
        self
    }
}

impl std::fmt::Display for ClausePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.root)?;
        for step in &self.steps {
            write!(f, " → {step}")?;
        }
        Ok(())
    }
}

/// One or more paths, ordered and without repeats.
///
/// Ordering and deduplication belong here rather than at each call site, because an answer with several
/// witnesses is the ordinary case for a fact established by any of a dialect's signals - and a diagnostic that
/// lists the same witness twice, or lists them in whatever order a hash map produced, reads as noise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSet(Vec<ClausePath>);

impl EvidenceSet {
    /// From at least one path. `None` where there is none, because an answer with no evidence is not an answer
    /// this type describes - that is the absence of one, which the caller expresses with `Option`.
    pub fn of(mut paths: Vec<ClausePath>) -> Option<Self> {
        if paths.is_empty() {
            return None;
        }
        paths.sort();
        paths.dedup();
        Some(Self(paths))
    }

    /// One path.
    pub fn one(path: ClausePath) -> Self {
        Self(vec![path])
    }

    pub fn paths(&self) -> &[ClausePath] {
        &self.0
    }
}

impl std::fmt::Display for EvidenceSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rendered: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        f.write_str(&rendered.join(", "))
    }
}

/// An answer together with the declarations that produced it.
///
/// Deliberately **not** used with `bool`. A negative answer almost never means "a clause said false"; it means
/// no clause answered at all, and there is no evidence to carry - so absence is `Option<Verdict<T>>` and a
/// `Verdict` always has at least one witness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict<T> {
    pub value: T,
    pub evidence: EvidenceSet,
}

impl<T> Verdict<T> {
    pub fn new(value: T, evidence: EvidenceSet) -> Self {
        Self { value, evidence }
    }

    /// One witness.
    pub fn from_one(value: T, path: ClausePath) -> Self {
        Self {
            value,
            evidence: EvidenceSet::one(path),
        }
    }
}

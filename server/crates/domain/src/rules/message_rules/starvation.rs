use super::*;

/// The carriers a rule's emission owns **together**, where losing one loses the whole emission.
///
/// Empty for an ordinary reading, whose emission owns the one carrier it read - there, a lower-ranked rule
/// taking that carrier means the two take turns, which is what ranks are for. Non-empty for a reading that is
/// *all or nothing*, where an emission is accepted only if its whole ownership set is free:
///
/// | Reading | Owned together |
/// | --- | --- |
/// | `compose` with several members, or a sweep | every spelling of every member, a member's fallback where the compose states a `where`, and a sweep's prefix less its `except` - `composed()` selects the **first present** with `find_map` and never retries a backup |
/// | `indexed_family` | the family's keys: each entry owns its own members, and an aggregate owns every entry |
/// | `overlay` | the base carrier and the overlay's, which are joined into one observation |
///
/// The distinction matters because the *conditional* excuse - "a gated rule and an ungated one take turns, and
/// the ranks decide" - is sound for a single-carrier reading and false here. A rule that takes one member of a
/// composed reading does not merely go first: the composed emission is dropped whole, so the carriers the
/// taker never wanted end up owned by **nobody** and their content disappears from the feed. Silently, and
/// neither rule looks wrong on its own.
pub(super) fn owned_all_or_nothing(rule: &CompiledMessageRule) -> Vec<Owned> {
    if let Some(set) = &rule.branch_set {
        // A branch leaf is a rule of its own, and each leaf's reading is all-or-nothing on its own terms.
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .flat_map(owned_all_or_nothing)
            .collect();
    }
    let mut out: Vec<Owned> = Vec::new();
    if let Some(compose) = &rule.compose {
        // A compose that can own **several** carriers: several members, or a sweep, which owns every key under
        // its prefix that it reads. One member reading several spellings takes exactly one of them, so there is
        // nothing another rule can take half of. Each spelling counts, and a swept prefix less the names it
        // excepts.
        //
        // A member's conditional **fallback** gives way instead: where another rule owns its key, the compose
        // drops that member and keeps the rest (`given_way`), so taking it starves no member another key
        // supplied: the compose is dropped only when no named member is left, and the carriers it then leaves
        // unowned are swept ones - the metadata of a message whose content its taker now holds. Unless the
        // compose states a `where`: what is left is asked it again and may fail, and nothing here can prove it
        // will not, so there the fallback is counted like a member's own spelling.
        if compose.members.len() > 1
            || compose
                .members
                .iter()
                .any(|member| member.spec.sweep_prefix.is_some())
        {
            for member in &compose.members {
                let spec = &member.spec;
                out.extend(spec.from_any_of.iter().map(|key| Owned::exact(key)));
                if !compose.require.is_empty() {
                    out.extend(
                        spec.fallback
                            .iter()
                            .map(|fallback| Owned::exact(&fallback.from)),
                    );
                }
                if let Some(prefix) = &spec.sweep_prefix {
                    out.push(Owned {
                        pattern: CarrierPattern::Prefix(prefix.clone()),
                        except: spec.except.clone(),
                    });
                }
            }
        }
    }
    // A dotted family reads its root and every key below it into one object, owned together.
    if let Some(family) = &rule.read.family {
        out.push(Owned {
            pattern: CarrierPattern::Prefix(family.clone()),
            except: Vec::new(),
        });
    }
    if let Some(family) = &rule.read.indexed_family {
        out.push(Owned {
            pattern: CarrierPattern::Prefix(format!("{family}.")),
            except: Vec::new(),
        });
    }
    if let Some(overlay) = &rule.read.overlay {
        out.push(Owned::exact(&overlay.from));
        if let Some(attribute) = rule.read.attribute() {
            out.push(Owned::exact(attribute));
        }
    }
    out
}

/// A carrier an all-or-nothing reading owns, less the names under a prefix it declares it does not read.
pub(super) struct Owned {
    pub(super) pattern: CarrierPattern,
    /// Suffixes under a `Prefix` pattern the reading leaves alone - a sweep's `except`.
    pub(super) except: Vec<String>,
}

impl Owned {
    fn exact(key: &str) -> Self {
        Self {
            pattern: CarrierPattern::Exact(key.to_string()),
            except: Vec::new(),
        }
    }

    /// Whether a carrier another rule reads is one this reading owns.
    pub(super) fn taken_by(&self, consumed: &CarrierPattern) -> bool {
        if !self.pattern.overlaps(consumed) {
            return false;
        }
        match (&self.pattern, consumed) {
            (CarrierPattern::Prefix(prefix), CarrierPattern::Exact(name)) => name
                .strip_prefix(prefix.as_str())
                .is_none_or(|suffix| !self.except.iter().any(|skip| skip == suffix)),
            _ => true,
        }
    }
}

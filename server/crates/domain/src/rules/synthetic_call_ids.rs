//! The declared synthetic call ids (`synthetic_call_ids`): the separator each closed-form template states.

use super::diagnostics::ClauseDefect;
use super::schema::RuleFile;

/// The separators of the declared synthetic call ids, refusing a template outside the closed form.
pub(super) fn compile(files: &[RuleFile]) -> Result<Vec<String>, ClauseDefect> {
    let mut out: Vec<String> = Vec::new();
    for file in files {
        for entry in &file.synthetic_call_ids {
            let separator = entry
                .template
                .strip_prefix("{name}")
                .and_then(|rest| rest.strip_suffix("{index}"))
                .filter(|separator| !separator.is_empty() && !separator.contains(['{', '}']))
                .ok_or_else(|| {
                    ClauseDefect::new(&[&entry.id], Some(&file.id), format!(
                        "synthetic call id `{}` in `{}` declares `{}`, which is not `{{name}}<separator>{{index}}` \
                         with a non-empty separator",
                        entry.id, file.id, entry.template
                    ))
                })?;
            if out.iter().any(|seen| seen == separator) {
                return Err(ClauseDefect::new(
                    &[&entry.id],
                    Some(&file.id),
                    format!(
                        "synthetic call id `{}` in `{}` restates a separator another declaration states",
                        entry.id, file.id
                    ),
                ));
            }
            out.push(separator.to_string());
        }
    }
    Ok(out)
}

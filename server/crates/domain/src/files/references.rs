use sideseat_core::utils::file_uri::{FILE_URI_PREFIX, is_file_uri};

/// Collect every file reference in a string, including references embedded in prose or JSON text.
pub fn collect_file_references_in_str(value: &str, references: &mut Vec<String>) {
    let Some(mut start) = value.find(FILE_URI_PREFIX) else {
        return;
    };

    loop {
        let body = &value[start + FILE_URI_PREFIX.len()..];
        let taken = body
            .find(|character: char| {
                !(character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '/' | '.' | ':' | '+'))
            })
            .unwrap_or(body.len());
        let mut candidate = &value[start..start + FILE_URI_PREFIX.len() + taken];
        while candidate.ends_with(['.', ':']) {
            candidate = &candidate[..candidate.len() - 1];
        }
        if is_file_uri(candidate) {
            references.push(candidate.to_string());
        }

        let resume = start + FILE_URI_PREFIX.len() + taken;
        match value[resume..].find(FILE_URI_PREFIX) {
            Some(next) => start = resume + next,
            None => break,
        }
    }
}

/// Whether a character continues a reference's token, as [`collect_file_references_in_str`] reads one.
fn continues_reference(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '/' | '.' | ':' | '+')
}

/// `value` with every whole occurrence of the reference `uri` replaced by `replacement`: an occurrence the scanner
/// would read as a longer reference - `uri` followed by more of a token, as a hash beginning with a shorter one is -
/// is left as it is, so replacing one reference never corrupts another. A trailing `.` or `:` ends a token, as the
/// scanner trims it. `None` where nothing was replaced.
pub fn replace_file_reference(value: &str, uri: &str, replacement: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut replaced = false;
    while let Some(at) = rest.find(uri) {
        let after = &rest[at + uri.len()..];
        // The token the scanner would read here: more token characters, less any trailing `.` or `:`.
        let tail: String = after
            .chars()
            .take_while(|c| continues_reference(*c))
            .collect();
        let whole = tail.trim_end_matches(['.', ':']).is_empty();
        out.push_str(&rest[..at]);
        if whole {
            out.push_str(replacement);
            replaced = true;
        } else {
            out.push_str(uri);
        }
        rest = after;
    }
    out.push_str(rest);
    replaced.then_some(out)
}

#[cfg(test)]
mod replace_tests {
    use super::replace_file_reference;

    /// A reference is replaced whole; a longer one that begins with it is not touched.
    #[test]
    fn only_a_whole_reference_is_replaced() {
        let short = "#!B64!#image/png::a";
        let long = format!("#!B64!#image/png::a{}", "b".repeat(63));
        let text = format!("see {short} and {long}.");
        assert_eq!(
            replace_file_reference(&text, short, "[note]").as_deref(),
            Some(format!("see [note] and {long}.").as_str())
        );
        assert_eq!(
            replace_file_reference(&format!("{short}."), short, "[note]").as_deref(),
            Some("[note].")
        );
        assert_eq!(replace_file_reference(&long, short, "[note]"), None);
    }
}

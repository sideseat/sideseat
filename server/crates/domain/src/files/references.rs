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

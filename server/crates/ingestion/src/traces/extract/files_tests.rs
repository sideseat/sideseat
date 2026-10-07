//! File-extraction tests.
include!("files_tests_parts/part_01_tests.rs");
include!("files_tests_parts/part_02_tests.rs");
include!("files_tests_parts/part_03_tests.rs");

/// The members the assets declare as media holders and as prose are exactly the retired lists, which are kept
/// here as the oracle.
#[test]
fn the_declared_media_and_prose_members_are_the_lists_they_replace() {
    const RETIRED_EXTRACTABLE: &[&str] = &[
        "data",
        "bytes",
        "base64",
        "b64",
        "url",
        "image_url",
        "image_data",
        "audio_data",
        "file_data",
    ];
    const RETIRED_PROTECTED: &[&str] = &[
        "text",
        "content",
        "message",
        "name",
        "description",
        "thinking",
        "reasoning",
        "title",
        "prompt",
        "system",
    ];
    let members = &sideseat_domain::rules::ruleset().message_members;
    for member in RETIRED_EXTRACTABLE {
        assert!(members.may_hold_media_bytes(member), "{member}");
        assert!(!members.holds_prose(member), "{member}");
    }
    for member in RETIRED_PROTECTED {
        assert!(members.holds_prose(member), "{member}");
        assert!(!members.may_hold_media_bytes(member), "{member}");
    }
    for other in ["source", "type", "mime_type", "parts", "Data", "data.0"] {
        assert!(
            !members.may_hold_media_bytes(other) && !members.holds_prose(other),
            "{other}"
        );
    }
    assert_eq!(
        members.media_byte_members().count(),
        RETIRED_EXTRACTABLE.len()
    );
    assert_eq!(members.prose_members().count(), RETIRED_PROTECTED.len());
}

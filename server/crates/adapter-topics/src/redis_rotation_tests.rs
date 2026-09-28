use super::cursor::{Rotation, StreamId, advance_rotation};

fn id(millis: u64, sequence: u64) -> StreamId {
    StreamId { millis, sequence }
}

/// A rotation round-trips through its stored form, front position included.
///
/// The stored value carries two facts - where to resume and where this rotation ends - because a cursor
/// alone cannot express a bounded sweep: wrapping on a short page never wraps while the list grows at the
/// tail. A parse that silently dropped the end would restore the unbounded behaviour, so the encoding is
/// pinned here.
#[test]
fn a_rotation_round_trips_through_its_stored_form() {
    for rotation in [
        Rotation {
            position: Some(id(50, 3)),
            end: id(900, 0),
        },
        Rotation {
            position: None,
            end: id(900, 0),
        },
    ] {
        let encoded = rotation.to_string();
        assert_eq!(
            Rotation::parse(&encoded),
            Some(rotation),
            "round trip failed for {encoded}"
        );
    }
}

/// The front of the list is `-`, distinct from any real id.
#[test]
fn the_front_position_is_encoded_as_a_dash() {
    let rotation = Rotation {
        position: None,
        end: id(7, 0),
    };
    assert_eq!(rotation.to_string(), "-|7-0");
}

/// A full page whose end passes the rotation's endpoint completes the rotation.
///
/// This is the condition that makes the sweep bounded. Without it a list growing at the tail keeps
/// returning full pages, the rotation never completes, and entries behind the cursor are never revisited -
/// the original starvation. Verified by disabling the check and watching this fail.
#[test]
fn passing_the_endpoint_completes_the_rotation() {
    let rotation = Rotation {
        position: Some(id(10, 0)),
        end: id(100, 0),
    };
    // Full page ending exactly at the endpoint: the next position is past it, so the rotation is done.
    assert_eq!(advance_rotation(rotation, 5, 5, Some(id(100, 0))), None);
    // Full page ending before the endpoint: resume past what was examined.
    assert_eq!(
        advance_rotation(rotation, 5, 5, Some(id(50, 0))),
        Some(Rotation {
            position: Some(id(50, 1)),
            end: id(100, 0),
        })
    );
}

/// A short page completes the rotation: the range held nothing more.
#[test]
fn a_short_page_completes_the_rotation() {
    let rotation = Rotation {
        position: None,
        end: id(100, 0),
    };
    assert_eq!(advance_rotation(rotation, 2, 5, Some(id(30, 0))), None);
}

/// The advance does not depend on whether anything was claimable.
///
/// A cursor that waits for an entry can be pinned there by a peer repeatedly claiming and abandoning it,
/// starving everything after it - which is why eligibility is not an input here at all.
#[test]
fn the_advance_ignores_eligibility() {
    let rotation = Rotation {
        position: None,
        end: id(100, 0),
    };
    assert_eq!(
        advance_rotation(rotation, 5, 5, Some(id(40, 2))),
        Some(Rotation {
            position: Some(id(40, 3)),
            end: id(100, 0),
        })
    );
}

/// A value that is not a rotation is refused rather than half-read.
///
/// An unparseable cursor makes the pass start a fresh rotation, which is safe. Accepting a partial parse
/// would instead resume from a position with no endpoint, which is the unbounded sweep.
#[test]
fn a_malformed_rotation_is_refused() {
    for raw in [
        "",
        "50-0",
        "50-0|",
        "|900-0",
        "notanid|900-0",
        "50-0|notanid",
    ] {
        assert_eq!(Rotation::parse(raw), None, "{raw} should not parse");
    }
}

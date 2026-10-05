//! Delivery must not change the answer: every fixture, delivered differently, reconstructs the same views.
//!
//! The golden tests check a few of these properties on one fixture per suite; this checks them on every
//! fixture, because a defect that depends on arrival order or batching shows up in whichever capture
//! happens to have the shape, not in the suite's first sample. It replays every fixture several times, so
//! it runs in `make test`, not in the inner loop.
//!
//! - **arrival order**: the rows reversed;
//! - **re-delivery**: every row twice, as a retried export sends it;
//! - **batch splitting**: every span exported in a request of its own;
//! - **clock offset**: every timestamp of every request moved by the same amount, as a host with a skewed
//!   clock reports it. Fixtures with log exports are excluded, because their log records keep their own
//!   clock and a skew between the two is a different scenario.
//!
//! - **framework release**: a fixture captured on another release of its framework
//!   (`<producer>/<mode>@<version>/<scenario>`, replayed from the same recorded responses) holds the same
//!   conversation as the current release's - every trace view, session view and the feed, block by block.
//!   Span names may differ; what the model said did not.
//!
//! A sub-millisecond jitter is not asserted: the pipeline's stated tolerance (`SPAN_CLOCK_SKEW`, 1 ms)
//! decides ties between spans, so moving a span across it legitimately changes what it decides. Log
//! exports arriving late or twice are covered by `a_resent_log_export_attaches_once`: logs join at read
//! time, so their arrival order is not an input. Native and SDK parity is
//! `framework_sdk_and_native_conversations_are_identical`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use sideseat_ports::types::MessageSpanRow;

use super::{Violation, ViolationView};
use crate::{attach_log_messages, build_golden, decode_request, describe_diff, normalize_for_test};

type Rows = Vec<(String, MessageSpanRow)>;

fn rows_of(requests: &[ExportTraceServiceRequest], paths: &[PathBuf]) -> Rows {
    let pricing =
        sideseat_domain::pricing::PricingService::init_for_test().expect("offline pricing service");
    let mut rows: Rows = requests
        .iter()
        .flat_map(|request| normalize_for_test(request, &pricing))
        .collect();
    attach_log_messages(paths, &mut rows);
    rows
}

/// One request per span, each keeping its resource and scope.
fn split(requests: &[ExportTraceServiceRequest]) -> Vec<ExportTraceServiceRequest> {
    let mut out = Vec::new();
    for request in requests {
        for resource in &request.resource_spans {
            for scope in &resource.scope_spans {
                for span in &scope.spans {
                    let mut scope = scope.clone();
                    scope.spans = vec![span.clone()];
                    let mut resource = resource.clone();
                    resource.scope_spans = vec![scope];
                    out.push(ExportTraceServiceRequest {
                        resource_spans: vec![resource],
                    });
                }
            }
        }
    }
    out
}

/// Every span and event timestamp moved by the same amount.
fn offset(requests: &[ExportTraceServiceRequest], nanos: u64) -> Vec<ExportTraceServiceRequest> {
    let mut out = requests.to_vec();
    for request in &mut out {
        for resource in &mut request.resource_spans {
            for scope in &mut resource.scope_spans {
                for span in &mut scope.spans {
                    span.start_time_unix_nano += nanos;
                    span.end_time_unix_nano += nanos;
                    for event in &mut span.events {
                        event.time_unix_nano += nanos;
                    }
                }
            }
        }
    }
    out
}

fn has_logs(paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .filter_map(|p| p.parent())
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .any(|e| e.file_name().to_string_lossy().starts_with("logs-"))
}

/// What a conversation is, independent of how a framework version names its spans: every trace
/// view, session view and the feed, as role, block type and full-content digest.
fn conversation(golden: &crate::Golden) -> Vec<Vec<(String, String, String)>> {
    let project = |view: &crate::GoldenView| {
        view.messages
            .iter()
            .map(|m| {
                (
                    m.role.clone(),
                    m.entry_type.clone(),
                    m.content_digest.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut sessions: Vec<_> = golden.session_views.values().map(project).collect();
    sessions.sort();
    let mut out: Vec<_> = golden.trace_views.values().map(project).collect();
    out.extend(sessions);
    out.push(project(&golden.feed_view));
    out
}

/// A fixture captured on another release of its framework: `<producer>/<mode>@<version>/<scenario>`,
/// with the current release's `<producer>/<mode>/<scenario>`.
fn current_release_of(label: &str) -> Option<String> {
    let mut parts = label.splitn(3, '/');
    let (producer, mode, scenario) = (parts.next()?, parts.next()?, parts.next()?);
    let (mode, _version) = mode.split_once('@')?;
    Some(format!("{producer}/{mode}/{scenario}"))
}

/// The ways one fixture's views changed under a different delivery, and its views as delivered.
fn differences(label: &str, paths: &[PathBuf]) -> (Vec<Violation>, crate::Golden) {
    let requests: Vec<ExportTraceServiceRequest> =
        paths.iter().map(|p| decode_request(p)).collect();
    let rows = rows_of(&requests, paths);
    let baseline = build_golden(label, paths, &rows).golden;
    let mut variants: Vec<(&str, Rows)> = vec![
        (
            "invariance.arrival_order",
            rows.iter().rev().cloned().collect(),
        ),
        (
            "invariance.re_delivery",
            rows.iter().chain(rows.iter()).cloned().collect(),
        ),
        (
            "invariance.batch_splitting",
            rows_of(&split(&requests), paths),
        ),
    ];
    if !has_logs(paths) {
        // Seven hours and thirteen minutes: crosses no day or hour boundary a format could round to.
        let skew = (7 * 3600 + 13 * 60) * 1_000_000_000;
        variants.push((
            "invariance.clock_offset",
            rows_of(&offset(&requests, skew), paths),
        ));
    }
    let found = variants
        .into_iter()
        .filter_map(|(name, rows)| {
            assert!(
                super::DELIVERY_FAMILIES.contains(&name),
                "{name} is not registered"
            );
            let golden = build_golden(label, paths, &rows).golden;
            (golden != baseline).then(|| {
                let diff = describe_diff(label, &baseline, &golden);
                let summary: Vec<&str> = diff.lines().skip(1).map(str::trim).collect();
                let mut violation = Violation::new(
                    ViolationView::Delivery,
                    name,
                    "views",
                    // The digest keeps the fingerprint sensitive to the whole difference.
                    format!(
                        "the views change ({}): {}",
                        crate::content_digest(&serde_json::Value::String(diff.clone())),
                        summary.join(" | ").chars().take(200).collect::<String>()
                    ),
                );
                violation.fixture = label.to_string();
                violation
            })
        })
        .collect();
    (found, baseline)
}

#[test]
fn no_delivery_or_framework_release_changes_a_conversation() {
    let fixtures = crate::discover_fixtures();
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let goldens = Mutex::new(std::collections::BTreeMap::new());
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((label, paths)) = fixtures.get(index) else {
                        break;
                    };
                    let (found, golden) = differences(label, paths);
                    failures.lock().expect("no worker panicked").extend(found);
                    goldens
                        .lock()
                        .expect("no worker panicked")
                        .insert(label.clone(), golden);
                }
            });
        }
    });
    let mut observed = failures.into_inner().expect("no worker panicked");
    // The same scenario, replayed from the same recorded responses on another release of the
    // framework, is the same conversation: what the model said did not change.
    let goldens = goldens.into_inner().expect("no worker panicked");
    for (label, golden) in &goldens {
        let Some(current) = current_release_of(label).and_then(|c| goldens.get(&c)) else {
            continue;
        };
        if conversation(golden) != conversation(current) {
            let diff = describe_diff(label, current, golden);
            let summary: Vec<&str> = diff
                .lines()
                .skip(1)
                .map(str::trim)
                .filter(|l| !l.contains("span view"))
                .collect();
            let mut violation = Violation::new(
                ViolationView::Delivery,
                "invariance.framework_version",
                "conversation",
                format!(
                    "differs from {} ({}): {}",
                    current.label,
                    crate::content_digest(&serde_json::Value::String(format!(
                        "{:?}",
                        conversation(golden)
                    ))),
                    summary.join(" | ").chars().take(200).collect::<String>()
                ),
            );
            violation.fixture = label.clone();
            observed.push(violation);
        }
    }
    observed.sort();
    let problems = super::ledger_problems(&observed, true);
    assert!(
        problems.is_empty(),
        "{} disagreement(s) between delivery variations and the ledger:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

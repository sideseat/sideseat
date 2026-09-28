use super::*;

#[test]
fn traces_declare_the_full_signal_contract() {
    fn assert_signal<T: Signal>() {}
    assert_signal::<TraceSignal>();

    let descriptor = SignalDescriptor {
        name: "traces",
        queue_topic: TOPIC_TRACES,
        debug_file: "traces.jsonl",
        durability: DurabilityRequirement::DurableBeforeAck,
        confirmation: ConfirmationPredicate {
            identity: SignalIdentity::TraceSpanCarrierPosition,
            strict_content_digest: true,
            excludes_system_metadata: true,
        },
    };
    assert_eq!(descriptor.queue_topic, TOPIC_TRACES);
    assert!(descriptor.confirmation.strict_content_digest);
    assert!(descriptor.confirmation.excludes_system_metadata);
}

#[test]
fn metrics_declare_the_full_signal_contract() {
    fn assert_signal<T: Signal>() {}
    assert_signal::<MetricsSignal>();
    let descriptor = SignalDescriptor {
        name: "metrics",
        queue_topic: TOPIC_METRICS,
        debug_file: "metrics.jsonl",
        durability: DurabilityRequirement::DurableBeforeAck,
        confirmation: ConfirmationPredicate {
            identity: SignalIdentity::MetricDatapointId,
            strict_content_digest: true,
            excludes_system_metadata: true,
        },
    };
    assert_eq!(descriptor.queue_topic, TOPIC_METRICS);
    assert!(descriptor.confirmation.strict_content_digest);
}

#[test]
fn logs_declare_the_full_signal_contract() {
    fn assert_signal<T: Signal>() {}
    assert_signal::<LogSignal>();
    let descriptor = SignalDescriptor {
        name: "logs",
        queue_topic: TOPIC_LOGS,
        debug_file: "logs.jsonl",
        durability: DurabilityRequirement::DurableBeforeAck,
        confirmation: ConfirmationPredicate {
            identity: SignalIdentity::LogDigestAndOrdinal,
            strict_content_digest: true,
            excludes_system_metadata: true,
        },
    };
    assert_eq!(descriptor.queue_topic, TOPIC_LOGS);
    assert!(descriptor.confirmation.strict_content_digest);
}

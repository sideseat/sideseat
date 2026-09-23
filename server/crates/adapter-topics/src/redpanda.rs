//! RedPanda/Kafka implementation of the queue port.
//!
//! Kafka has no Redis-style per-message claim or consumed-entry trim. Consumer-group
//! rebalance recovers abandoned partitions, while acknowledgements advance only the
//! highest contiguous completed offset.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_stream::stream;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use parking_lot::Mutex;
use rdkafka::Message;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, DefaultConsumerContext, StreamConsumer};
use rdkafka::error::RDKafkaErrorCode;
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use serde::Serialize;
use sideseat_core::core::config::RedpandaConfig;
use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend, TopicError,
};

use crate::ack_window::AckWindow;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const RETENTION_MONITOR_INTERVAL: Duration = Duration::from_secs(30);

type KafkaConsumer = StreamConsumer<DefaultConsumerContext>;

#[derive(Debug)]
struct AckState {
    window: AckWindow,
    last_committed: Option<u64>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PartitionKey {
    topic: String,
    group: String,
    partition: i32,
}

#[derive(Debug, Serialize)]
struct DeadLetter<'a> {
    original_id: &'a str,
    reason: &'a str,
    payload: &'a [u8],
}

/// RedPanda-backed queue and broadcast transport.
pub struct RedpandaTopicBackend {
    brokers: String,
    producer: FutureProducer,
    admin: AdminClient<DefaultClientContext>,
    partitions: i32,
    replication_factor: i32,
    retention_ms: u64,
    retention_warning_ms: u64,
    topics: Arc<DashSet<String>>,
    consumers: Arc<DashMap<(String, String), Arc<KafkaConsumer>>>,
    ack_windows: Arc<DashMap<PartitionKey, Mutex<AckState>>>,
    pending_timestamps: Arc<DashMap<(PartitionKey, u64), i64>>,
    retention_risk: Arc<AtomicBool>,
    monitor_started: AtomicBool,
}

impl RedpandaTopicBackend {
    pub async fn new(config: &RedpandaConfig) -> Result<Self, TopicError> {
        let mut client = base_client_config(&config.brokers);
        client
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("delivery.timeout.ms", "10000")
            .set("request.timeout.ms", "5000");
        let producer: FutureProducer = client
            .create()
            .map_err(|error| TopicError::Connection(error.to_string()))?;

        let admin: AdminClient<DefaultClientContext> = base_client_config(&config.brokers)
            .create()
            .map_err(|error| TopicError::Connection(error.to_string()))?;

        producer
            .client()
            .fetch_metadata(None, CLIENT_TIMEOUT)
            .map_err(|error| TopicError::Connection(error.to_string()))?;

        Ok(Self {
            brokers: config.brokers.clone(),
            producer,
            admin,
            partitions: config.partitions,
            replication_factor: config.replication_factor,
            retention_ms: config.retention_ms,
            retention_warning_ms: config.retention_warning_ms,
            topics: Arc::new(DashSet::new()),
            consumers: Arc::new(DashMap::new()),
            ack_windows: Arc::new(DashMap::new()),
            pending_timestamps: Arc::new(DashMap::new()),
            retention_risk: Arc::new(AtomicBool::new(false)),
            monitor_started: AtomicBool::new(false),
        })
    }

    async fn ensure_topic(&self, topic: &str) -> Result<(), TopicError> {
        if self.topics.contains(topic) {
            return Ok(());
        }

        let retention_ms = self.retention_ms.to_string();
        let topic_spec = NewTopic::new(
            topic,
            self.partitions,
            TopicReplication::Fixed(self.replication_factor),
        )
        .set("cleanup.policy", "delete")
        .set("retention.ms", &retention_ms);
        let results = self
            .admin
            .create_topics(
                [&topic_spec],
                &AdminOptions::new().operation_timeout(Some(CLIENT_TIMEOUT)),
            )
            .await
            .map_err(|error| TopicError::Stream(error.to_string()))?;
        for result in results {
            match result {
                Ok(_) | Err((_, RDKafkaErrorCode::TopicAlreadyExists)) => {}
                Err((name, code)) => {
                    return Err(TopicError::Stream(format!(
                        "create RedPanda topic {name}: {code}"
                    )));
                }
            }
        }
        self.topics.insert(topic.to_string());
        Ok(())
    }

    fn consumer(&self, group: &str, consumer: &str) -> Result<KafkaConsumer, TopicError> {
        let mut config = base_client_config(&self.brokers);
        config
            .set("group.id", group)
            .set("client.id", consumer)
            .set("enable.auto.commit", "false")
            .set("enable.auto.offset.store", "false")
            .set("auto.offset.reset", "earliest")
            .set("partition.assignment.strategy", "cooperative-sticky")
            .set("session.timeout.ms", "10000")
            .set("max.poll.interval.ms", "300000");
        config
            .create()
            .map_err(|error| TopicError::ConsumerGroup(error.to_string()))
    }

    fn start_retention_monitor(&self) {
        if self.monitor_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending = Arc::clone(&self.pending_timestamps);
        let consumers = Arc::clone(&self.consumers);
        let risk = Arc::clone(&self.retention_risk);
        let retention_ms = self.retention_ms;
        let warning_ms = self.retention_warning_ms;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(RETENTION_MONITOR_INTERVAL);
            loop {
                interval.tick().await;
                let now = unix_millis();
                let oldest_age = pending
                    .iter()
                    .map(|entry| now.saturating_sub(*entry.value()).max(0) as u64)
                    .max()
                    .unwrap_or(0);
                let threshold_age = retention_ms.saturating_sub(warning_ms);
                let consumer_snapshot: Vec<_> = consumers
                    .iter()
                    .map(|entry| {
                        (
                            entry.key().0.clone(),
                            entry.key().1.clone(),
                            Arc::clone(entry.value()),
                        )
                    })
                    .collect();
                let broker_risk = consumer_snapshot.iter().any(|(topic, group, consumer)| {
                    match consumer_retention_risk(consumer, topic, threshold_age) {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::error!(
                                topic,
                                group,
                                %error,
                                "failed to measure RedPanda consumer lag against retention"
                            );
                            true
                        }
                    }
                });
                let at_risk = oldest_age >= threshold_age || broker_risk;
                risk.store(at_risk, Ordering::Release);
                if at_risk {
                    tracing::error!(
                        oldest_pending_ms = oldest_age,
                        retention_ms,
                        "RedPanda consumer lag is approaching topic retention"
                    );
                }
            }
        });
    }

    async fn produce(&self, topic: &str, key: &str, payload: &[u8]) -> Result<String, TopicError> {
        self.ensure_topic(topic).await?;
        let delivery = self
            .producer
            .send(
                FutureRecord::to(topic).key(key).payload(payload),
                Timeout::After(CLIENT_TIMEOUT),
            )
            .await
            .map_err(|(error, _)| TopicError::Stream(error.to_string()))?;
        Ok(format_message_id(delivery.partition, delivery.offset))
    }

    fn complete_offsets(
        &self,
        topic: &str,
        group: &str,
        offsets: &HashMap<i32, Vec<u64>>,
    ) -> Result<(), TopicError> {
        let consumer = self
            .consumers
            .get(&(topic.to_string(), group.to_string()))
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| TopicError::ConsumerGroup(format!("no consumer for {topic}/{group}")))?;

        for (&partition, offsets) in offsets {
            let key = PartitionKey {
                topic: topic.to_string(),
                group: group.to_string(),
                partition,
            };
            let state = self.ack_windows.get(&key).ok_or_else(|| {
                TopicError::ConsumerGroup(format!(
                    "no delivered offset window for {topic}/{group}/{partition}"
                ))
            })?;
            let mut state = state.lock();
            for &offset in offsets {
                state.window.complete(offset);
            }
            let Some(next) = state.window.committable(state.last_committed) else {
                continue;
            };
            let next = i64::try_from(next)
                .map_err(|_| TopicError::ConsumerGroup("offset exceeds i64".into()))?;
            let mut list = TopicPartitionList::new();
            list.add_partition_offset(topic, partition, Offset::Offset(next))
                .map_err(|error| TopicError::ConsumerGroup(error.to_string()))?;
            consumer
                .commit(&list, CommitMode::Sync)
                .map_err(|error| TopicError::ConsumerGroup(error.to_string()))?;
            state.last_committed = Some(next as u64);
            self.pending_timestamps
                .retain(|(pending_key, offset), _| pending_key != &key || *offset >= next as u64);
        }
        Ok(())
    }
}

#[async_trait]
impl TopicBackend for RedpandaTopicBackend {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), TopicError> {
        self.produce(topic, "", payload).await.map(|_| ())
    }

    async fn subscribe(&self, topic: &str) -> Result<BroadcastSubscription, TopicError> {
        self.ensure_topic(topic).await?;
        let group = format!("sideseat-broadcast-{}", uuid::Uuid::new_v4());
        let consumer = Arc::new(self.consumer(&group, &group)?);
        consumer
            .subscribe(&[topic])
            .map_err(|error| TopicError::ConsumerGroup(error.to_string()))?;
        let receiver = stream! {
            loop {
                match consumer.recv().await {
                    Ok(message) => {
                        if let Some(payload) = message.payload() {
                            yield Ok(payload.to_vec());
                        }
                    }
                    Err(error) => yield Err(TopicError::Stream(error.to_string())),
                }
            }
        };
        Ok(BroadcastSubscription {
            receiver: Box::pin(receiver),
        })
    }

    async fn stream_publish(
        &self,
        topic: &str,
        partition_key: &str,
        payload: &[u8],
    ) -> Result<String, TopicError> {
        self.produce(topic, partition_key, payload).await
    }

    async fn stream_subscribe(
        &self,
        topic: &str,
        group: &str,
        consumer_name: &str,
    ) -> Result<StreamSubscription, TopicError> {
        self.ensure_topic(topic).await?;
        self.start_retention_monitor();
        let consumer = Arc::new(self.consumer(group, consumer_name)?);
        consumer
            .subscribe(&[topic])
            .map_err(|error| TopicError::ConsumerGroup(error.to_string()))?;
        self.consumers.insert(
            (topic.to_string(), group.to_string()),
            Arc::clone(&consumer),
        );
        let topic_name = topic.to_string();
        let group_name = group.to_string();
        let ack_windows = Arc::clone(&self.ack_windows);
        let pending = Arc::clone(&self.pending_timestamps);
        let receiver = stream! {
            loop {
                match consumer.recv().await {
                    Ok(message) => {
                        let offset = message.offset();
                        if offset < 0 {
                            yield Err(TopicError::Stream(format!("negative RedPanda offset {offset}")));
                            continue;
                        }
                        let offset = offset as u64;
                        let key = PartitionKey {
                            topic: topic_name.clone(),
                            group: group_name.clone(),
                            partition: message.partition(),
                        };
                        ack_windows.entry(key.clone()).or_insert_with(|| {
                            Mutex::new(AckState {
                                window: AckWindow::starting_at(offset),
                                last_committed: Some(offset),
                            })
                        });
                        let timestamp = message.timestamp().to_millis().unwrap_or_else(unix_millis);
                        pending.insert((key, offset), timestamp);
                        yield Ok(StreamMessage {
                            id: format_message_id(message.partition(), message.offset()),
                            partition: message.partition() as u32,
                            payload: message.payload().unwrap_or_default().to_vec(),
                        });
                    }
                    Err(error) => yield Err(TopicError::Stream(error.to_string())),
                }
            }
        };
        Ok(StreamSubscription {
            receiver: Box::pin(receiver),
        })
    }

    async fn stream_ack(&self, topic: &str, group: &str, id: &str) -> Result<(), TopicError> {
        self.stream_ack_batch(topic, group, &[id.to_string()]).await
    }

    async fn stream_ack_batch(
        &self,
        topic: &str,
        group: &str,
        ids: &[String],
    ) -> Result<(), TopicError> {
        let mut offsets: HashMap<i32, Vec<u64>> = HashMap::new();
        for id in ids {
            let (partition, offset) = parse_message_id(id)?;
            offsets.entry(partition).or_default().push(offset);
        }
        self.complete_offsets(topic, group, &offsets)
    }

    async fn stream_claim(
        &self,
        _topic: &str,
        _group: &str,
        _consumer: &str,
        _min_idle_ms: u64,
        _count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        Ok(Vec::new())
    }

    async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        let consumer = self
            .consumers
            .get(&(topic.to_string(), group.to_string()))
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| TopicError::ConsumerGroup(format!("no consumer for {topic}/{group}")))?;
        let metadata = consumer
            .fetch_metadata(Some(topic), CLIENT_TIMEOUT)
            .map_err(|error| TopicError::Stream(error.to_string()))?;
        let topic_metadata = metadata
            .topics()
            .iter()
            .find(|entry| entry.name() == topic)
            .ok_or_else(|| TopicError::Stream(format!("topic {topic} missing from metadata")))?;

        let mut requested = TopicPartitionList::new();
        for partition in topic_metadata.partitions() {
            requested
                .add_partition_offset(topic, partition.id(), Offset::Stored)
                .map_err(|error| TopicError::Stream(error.to_string()))?;
        }
        let committed = consumer
            .committed_offsets(requested, CLIENT_TIMEOUT)
            .map_err(|error| TopicError::Stream(error.to_string()))?;

        let mut length = 0_u64;
        let mut pending = 0_u64;
        for partition in topic_metadata.partitions() {
            let (low, high) = consumer
                .fetch_watermarks(topic, partition.id(), CLIENT_TIMEOUT)
                .map_err(|error| TopicError::Stream(error.to_string()))?;
            let low = low.max(0) as u64;
            let high = high.max(0) as u64;
            length = length.saturating_add(high.saturating_sub(low));
            let committed_offset = committed
                .find_partition(topic, partition.id())
                .and_then(|entry| match entry.offset() {
                    Offset::Offset(value) if value >= 0 => Some(value as u64),
                    _ => None,
                })
                .unwrap_or(low)
                .max(low);
            pending = pending.saturating_add(high.saturating_sub(committed_offset));
        }

        let now = unix_millis();
        let oldest_pending_ms = self
            .pending_timestamps
            .iter()
            .filter(|entry| entry.key().0.topic == topic && entry.key().0.group == group)
            .map(|entry| now.saturating_sub(*entry.value()).max(0) as u64)
            .max();
        Ok(StreamStats {
            length,
            pending,
            consumers: 1,
            oldest_pending_ms,
        })
    }

    async fn stream_dead_letter(
        &self,
        topic: &str,
        _group: &str,
        id: &str,
        reason: &str,
        payload: &[u8],
    ) -> Result<(), TopicError> {
        let dead_letter_topic = format!("{topic}.dead-letter");
        let encoded = rmp_serde::to_vec_named(&DeadLetter {
            original_id: id,
            reason,
            payload,
        })
        .map_err(|error| TopicError::Serialization(error.to_string()))?;
        self.produce(&dead_letter_topic, id, &encoded)
            .await
            .map(|_| ())
    }

    async fn health_check(&self) -> Result<(), TopicError> {
        self.producer
            .client()
            .fetch_metadata(None, CLIENT_TIMEOUT)
            .map_err(|error| TopicError::Connection(error.to_string()))?;
        if self.retention_risk.load(Ordering::Acquire) {
            return Err(TopicError::Stream(
                "oldest uncommitted RedPanda record is approaching retention".into(),
            ));
        }
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "redpanda"
    }

    fn is_durable(&self) -> bool {
        true
    }
}

fn base_client_config(brokers: &str) -> ClientConfig {
    let mut config = ClientConfig::new();
    config.set("bootstrap.servers", brokers);
    config
}

fn format_message_id(partition: i32, offset: i64) -> String {
    format!("{partition}:{offset}")
}

fn parse_message_id(id: &str) -> Result<(i32, u64), TopicError> {
    let (partition, offset) = id
        .split_once(':')
        .ok_or_else(|| TopicError::Stream(format!("invalid RedPanda message id {id}")))?;
    let partition = partition
        .parse::<i32>()
        .map_err(|error| TopicError::Stream(format!("invalid partition in {id}: {error}")))?;
    let offset = offset
        .parse::<u64>()
        .map_err(|error| TopicError::Stream(format!("invalid offset in {id}: {error}")))?;
    Ok((partition, offset))
}

fn consumer_retention_risk(
    consumer: &KafkaConsumer,
    topic: &str,
    threshold_age_ms: u64,
) -> Result<bool, TopicError> {
    let metadata = consumer
        .fetch_metadata(Some(topic), CLIENT_TIMEOUT)
        .map_err(|error| TopicError::Stream(error.to_string()))?;
    let topic_metadata = metadata
        .topics()
        .iter()
        .find(|entry| entry.name() == topic)
        .ok_or_else(|| TopicError::Stream(format!("topic {topic} missing from metadata")))?;
    let mut requested = TopicPartitionList::new();
    let threshold_timestamp =
        unix_millis().saturating_sub(i64::try_from(threshold_age_ms).unwrap_or(i64::MAX));
    let mut timestamps = TopicPartitionList::new();
    for partition in topic_metadata.partitions() {
        requested
            .add_partition_offset(topic, partition.id(), Offset::Stored)
            .map_err(|error| TopicError::Stream(error.to_string()))?;
        timestamps
            .add_partition_offset(topic, partition.id(), Offset::Offset(threshold_timestamp))
            .map_err(|error| TopicError::Stream(error.to_string()))?;
    }
    let committed = consumer
        .committed_offsets(requested, CLIENT_TIMEOUT)
        .map_err(|error| TopicError::Stream(error.to_string()))?;
    let thresholds = consumer
        .offsets_for_times(timestamps, CLIENT_TIMEOUT)
        .map_err(|error| TopicError::Stream(error.to_string()))?;

    for partition in topic_metadata.partitions() {
        let (low, high) = consumer
            .fetch_watermarks(topic, partition.id(), CLIENT_TIMEOUT)
            .map_err(|error| TopicError::Stream(error.to_string()))?;
        let committed_offset = committed
            .find_partition(topic, partition.id())
            .and_then(|entry| match entry.offset() {
                Offset::Offset(value) if value >= 0 => Some(value),
                _ => None,
            })
            .unwrap_or(low)
            .max(low);
        if committed_offset >= high {
            continue;
        }
        let threshold_offset = thresholds
            .find_partition(topic, partition.id())
            .and_then(|entry| match entry.offset() {
                Offset::Offset(value) if value >= 0 => Some(value),
                _ => None,
            });
        if threshold_offset.is_none_or(|offset| committed_offset < offset) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_round_trip() {
        let id = format_message_id(7, 42);
        assert_eq!(parse_message_id(&id).unwrap(), (7, 42));
    }

    #[test]
    fn malformed_message_ids_are_rejected() {
        assert!(parse_message_id("missing").is_err());
        assert!(parse_message_id("x:1").is_err());
        assert!(parse_message_id("1:-1").is_err());
    }
}

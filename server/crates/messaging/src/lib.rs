//! Typed messaging over the transport-neutral queue port.

use std::marker::PhantomData;
use std::sync::Arc;

use futures::StreamExt;
use prost::Message as ProstMessage;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend, TopicError,
};

/// Typed facade over a selected queue backend.
pub struct TopicService {
    backend: Arc<dyn TopicBackend>,
}

impl TopicService {
    #[must_use]
    pub fn new(backend: Arc<dyn TopicBackend>) -> Self {
        Self { backend }
    }

    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.backend_name()
    }

    #[must_use]
    pub fn is_durable(&self) -> bool {
        self.backend.is_durable()
    }

    #[must_use]
    pub fn stream_topic<T>(&self, name: &str, partition_key: fn(&T) -> String) -> StreamTopic<T>
    where
        T: ProstMessage + Default + Send + Sync + 'static,
    {
        StreamTopic {
            name: name.to_string(),
            backend: Arc::clone(&self.backend),
            partition_key,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn broadcast_topic<T>(&self, name: &str) -> BroadcastTopic<T>
    where
        T: Send + Sync + Serialize + DeserializeOwned + 'static,
    {
        BroadcastTopic {
            name: name.to_string(),
            backend: Arc::clone(&self.backend),
            marker: PhantomData,
        }
    }

    pub async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        self.backend.stream_stats(topic, group).await
    }

    pub async fn health_check(&self) -> Result<(), TopicError> {
        self.backend.health_check().await
    }

    pub async fn shutdown(&self) {
        self.backend.shutdown().await;
    }
}

pub struct StreamTopic<T>
where
    T: ProstMessage + Default + Send + Sync + 'static,
{
    name: String,
    backend: Arc<dyn TopicBackend>,
    partition_key: fn(&T) -> String,
    marker: PhantomData<T>,
}

impl<T> StreamTopic<T>
where
    T: ProstMessage + Default + Send + Sync + 'static,
{
    pub async fn publish(&self, message: &T) -> Result<String, TopicError> {
        self.backend
            .stream_publish(
                &self.name,
                &(self.partition_key)(message),
                &message.encode_to_vec(),
            )
            .await
    }

    pub async fn subscribe(
        &self,
        group: &str,
        consumer: &str,
    ) -> Result<StreamTopicSubscriber<T>, TopicError> {
        let subscription = self
            .backend
            .stream_subscribe(&self.name, group, consumer)
            .await?;
        Ok(StreamTopicSubscriber {
            name: self.name.clone(),
            group: group.to_string(),
            backend: Arc::clone(&self.backend),
            subscription,
            marker: PhantomData,
        })
    }
}

pub struct StreamTopicSubscriber<T>
where
    T: ProstMessage + Default + Send + Sync + 'static,
{
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
    subscription: StreamSubscription,
    marker: PhantomData<T>,
}

impl<T> StreamTopicSubscriber<T>
where
    T: ProstMessage + Default + Send + Sync + 'static,
{
    /// Receive a typed message together with the broker (or virtual) partition that delivered it.
    pub async fn recv_partitioned(&mut self) -> Result<(String, u32, T), TopicError> {
        let Some(message) = self.subscription.receiver.next().await else {
            return Err(TopicError::ChannelClosed);
        };
        let message = message?;
        let decoded =
            T::decode(message.payload.as_slice()).map_err(|error| TopicError::Undecodable {
                id: message.id.clone(),
                detail: error.to_string(),
                raw: message.payload,
            })?;
        Ok((message.id, message.partition, decoded))
    }

    #[must_use]
    pub fn acker(&self) -> StreamAcker {
        StreamAcker {
            name: self.name.clone(),
            group: self.group.clone(),
            backend: Arc::clone(&self.backend),
        }
    }

    #[must_use]
    pub fn claimer(&self) -> StreamClaimer {
        StreamClaimer {
            name: self.name.clone(),
            group: self.group.clone(),
            backend: Arc::clone(&self.backend),
        }
    }
}

#[derive(Clone)]
pub struct StreamAcker {
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
}

impl StreamAcker {
    pub async fn ack(&self, id: &str) -> Result<(), TopicError> {
        self.backend.stream_ack(&self.name, &self.group, id).await
    }

    pub async fn ack_batch(&self, ids: &[String]) -> Result<(), TopicError> {
        self.backend
            .stream_ack_batch(&self.name, &self.group, ids)
            .await
    }

    pub async fn dead_letter(
        &self,
        id: &str,
        reason: &str,
        payload: &[u8],
    ) -> Result<(), TopicError> {
        self.backend
            .stream_dead_letter(&self.name, &self.group, id, reason, payload)
            .await
    }
}

#[derive(Clone)]
pub struct StreamClaimer {
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
}

impl StreamClaimer {
    pub async fn claim(
        &self,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        self.backend
            .stream_claim(&self.name, &self.group, consumer, min_idle_ms, count)
            .await
    }

    pub async fn trim_consumed(&self) -> Result<u64, TopicError> {
        self.backend.stream_trim_consumed(&self.name).await
    }
}

pub struct BroadcastTopic<T>
where
    T: Send + Sync + Serialize + DeserializeOwned + 'static,
{
    name: String,
    backend: Arc<dyn TopicBackend>,
    marker: PhantomData<T>,
}

impl<T> BroadcastTopic<T>
where
    T: Send + Sync + Serialize + DeserializeOwned + 'static,
{
    pub async fn publish(&self, message: &T) -> Result<(), TopicError> {
        let payload = serde_json::to_vec(message)
            .map_err(|error| TopicError::Serialization(error.to_string()))?;
        self.backend.publish(&self.name, &payload).await
    }

    pub async fn subscribe(&self) -> Result<BroadcastTopicSubscriber<T>, TopicError> {
        Ok(BroadcastTopicSubscriber {
            subscription: self.backend.subscribe(&self.name).await?,
            marker: PhantomData,
        })
    }
}

pub struct BroadcastTopicSubscriber<T>
where
    T: Send + Sync + Serialize + DeserializeOwned + 'static,
{
    subscription: BroadcastSubscription,
    marker: PhantomData<T>,
}

impl<T> BroadcastTopicSubscriber<T>
where
    T: Send + Sync + Serialize + DeserializeOwned + 'static,
{
    pub async fn recv(&mut self) -> Result<T, TopicError> {
        let Some(payload) = self.subscription.receiver.next().await else {
            return Err(TopicError::ChannelClosed);
        };
        serde_json::from_slice(&payload?)
            .map_err(|error| TopicError::Serialization(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct NonCloneMessage;

    #[test]
    fn broadcast_messages_do_not_need_to_be_cloneable() {
        fn accepts_non_clone_message(service: &TopicService) {
            let _ = service.broadcast_topic::<NonCloneMessage>("compile-check");
        }

        let _: fn(&TopicService) = accepts_non_clone_message;
    }
}

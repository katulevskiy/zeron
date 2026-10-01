//! Who is talking to this engine: RPC calls in flight and live streams, from
//! the local IPC port and from other devices through the host relay.
//!
//! A cloud box stays awake while somebody is looking at it (docs/cloud.md
//! §4). Viewers don't hold runs or terminals open, but a device showing a
//! chat hosted here keeps streams open to this engine (workspace files,
//! diffs, terminal output), and every call it makes counts as activity.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use zeron_rpc::{RpcError, RpcReply, RpcService};

#[derive(Debug, Default)]
pub struct RpcActivity {
    live: AtomicUsize,
    last_call_ms: AtomicI64,
}

impl RpcActivity {
    /// Calls in flight plus open streams.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    /// Epoch ms of the last call (0 = none yet).
    pub fn last_call_ms(&self) -> i64 {
        self.last_call_ms.load(Ordering::Acquire)
    }

    fn begin(self: &Arc<Self>) -> LiveGuard {
        self.last_call_ms
            .fetch_max(crate::now_ms(), Ordering::AcqRel);
        self.live.fetch_add(1, Ordering::AcqRel);
        LiveGuard(self.clone())
    }
}

/// One call or stream; ends (and counts as activity once more) on drop.
struct LiveGuard(Arc<RpcActivity>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0
            .last_call_ms
            .fetch_max(crate::now_ms(), Ordering::AcqRel);
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// An [`RpcService`] that records [`RpcActivity`] around another.
pub struct TrackedRpc {
    inner: Arc<dyn RpcService>,
    activity: Arc<RpcActivity>,
}

impl TrackedRpc {
    pub fn wrap(inner: Arc<dyn RpcService>, activity: Arc<RpcActivity>) -> Arc<dyn RpcService> {
        Arc::new(Self { inner, activity })
    }
}

#[async_trait]
impl RpcService for TrackedRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        let guard = self.activity.begin();
        match self.inner.handle(method, params).await? {
            RpcReply::Stream(stream) => Ok(RpcReply::Stream(
                // The guard lives as long as the stream does.
                stream
                    .map(move |item| {
                        let _ = &guard;
                        item
                    })
                    .boxed(),
            )),
            value => Ok(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl RpcService for Echo {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            if method == "stream" {
                Ok(RpcReply::Stream(futures::stream::pending().boxed()))
            } else {
                Ok(RpcReply::Value(serde_json::json!(true)))
            }
        }
    }

    #[tokio::test]
    async fn streams_count_until_dropped_and_calls_mark_activity() {
        let activity = Arc::new(RpcActivity::default());
        let rpc = TrackedRpc::wrap(Arc::new(Echo), activity.clone());
        assert_eq!(activity.last_call_ms(), 0);
        rpc.handle("unary", serde_json::Value::Null).await.unwrap();
        assert_eq!(activity.live(), 0);
        assert!(activity.last_call_ms() > 0);
        let stream = rpc.handle("stream", serde_json::Value::Null).await.unwrap();
        assert_eq!(activity.live(), 1, "an open stream is a viewer");
        drop(stream);
        assert_eq!(activity.live(), 0);
    }
}

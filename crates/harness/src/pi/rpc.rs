//! Pi RPC is JSONL, not JSON-RPC 2.0. Responses and events share one ordered
//! channel: a get_state response is a barrier after preceding lifecycle events.
use crate::HarnessError;
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
};

const MAX_FRAME: usize = 32 * 1024 * 1024;
const QUEUE_BYTES: usize = 256 * 1024;
const CONTROL_HEADROOM: usize = 8 * 1024;
const WRITE_QUEUE: usize = 8;
const READ_QUEUE: usize = 2;

struct WriteFrame {
    bytes: Vec<u8>,
    // Includes the frame currently being written. One legal large frame may
    // exceed the target, but leaves headroom for small controls until flushed.
    _permit: OwnedSemaphorePermit,
}

struct ReadFrame {
    result: Result<Value, HarnessError>,
    _permit: Option<OwnedSemaphorePermit>,
}

pub(super) struct Incoming {
    receiver: mpsc::Receiver<ReadFrame>,
}

impl Incoming {
    pub async fn recv(&mut self) -> Option<Result<Value, HarnessError>> {
        self.receiver.recv().await.map(|frame| frame.result)
    }
}

// Bound serialization itself, including a prompt with many inline images.
// Checking the completed Vec would still allocate an arbitrarily large copy.
struct FrameBuffer(Vec<u8>);

pub(super) fn frame_size(frame: &Value) -> Result<usize, HarnessError> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, frame)
        .map_err(|error| HarnessError::Protocol(error.to_string()))?;
    Ok(counter.0.saturating_add(1))
}

impl std::io::Write for FrameBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (MAX_FRAME - 1).saturating_sub(self.0.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Pi RPC request exceeds 32 MiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct Client {
    writer: mpsc::Sender<WriteFrame>,
    budget: Arc<Semaphore>,
    errors: mpsc::Sender<ReadFrame>,
    failed: Arc<AtomicBool>,
    next_id: Arc<AtomicU64>,
}
impl Client {
    pub fn send(&self, frame: Value) -> Result<(), HarnessError> {
        let result = self.enqueue(frame);
        if let Err(error) = &result
            && !self.failed.swap(true, Ordering::Relaxed)
        {
            // Dialog responses are sent by detached UI tasks, which cannot
            // propagate a send failure to the actor directly. Report the first
            // failure through its ordered inbox so the provider is shut down.
            let errors = self.errors.clone();
            let message = error.to_string();
            tokio::spawn(async move {
                let _ = errors
                    .send(ReadFrame {
                        result: Err(HarnessError::Protocol(message)),
                        _permit: None,
                    })
                    .await;
            });
        }
        result
    }

    fn enqueue(&self, frame: Value) -> Result<(), HarnessError> {
        let mut bytes = FrameBuffer(Vec::new());
        serde_json::to_writer(&mut bytes, &frame)
            .map_err(|error| HarnessError::Protocol(error.to_string()))?;
        bytes.0.push(b'\n');
        let permit = self
            .budget
            .clone()
            .try_acquire_many_owned(bytes.0.len().min(QUEUE_BYTES - CONTROL_HEADROOM) as u32)
            .map_err(|_| HarnessError::Protocol("Pi pending writes exceed 256 KiB".into()))?;
        // The ordered Pi actor must keep reading stdout and handling shutdown.
        // Fail explicitly on overload rather than block it behind stalled stdin
        // or silently drop a prompt/control response.
        self.writer
            .try_send(WriteFrame {
                bytes: bytes.0,
                _permit: permit,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    HarnessError::Protocol("Pi stdin queue full".into())
                }
                mpsc::error::TrySendError::Closed(_) => {
                    HarnessError::Protocol("Pi stdin closed".into())
                }
            })
    }
    pub fn request(&self, mut frame: Value) -> Result<String, HarnessError> {
        let id = format!("zeron-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        frame["id"] = Value::String(id.clone());
        self.send(frame)?;
        Ok(id)
    }
}

pub(super) struct Transport {
    pub client: Client,
    pub incoming: Incoming,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}
impl Transport {
    pub fn new(
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        let (writer_tx, mut writer_rx) = mpsc::channel::<WriteFrame>(WRITE_QUEUE);
        let budget = Arc::new(Semaphore::new(QUEUE_BYTES));
        let read_budget = Arc::new(Semaphore::new(QUEUE_BYTES));
        let (tx, receiver) = mpsc::channel(READ_QUEUE);
        let client_errors = tx.clone();
        let errors = tx.clone();
        let writer = tokio::spawn(async move {
            let mut input = input;
            while let Some(frame) = writer_rx.recv().await {
                let result = async {
                    input.write_all(&frame.bytes).await?;
                    input.flush().await
                }
                .await;
                if let Err(e) = result {
                    let _ = errors
                        .send(ReadFrame {
                            result: Err(e.into()),
                            _permit: None,
                        })
                        .await;
                    break;
                }
            }
        });
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(output);
            let mut bytes = Vec::new();
            loop {
                // fill_buf bounds allocation before a hostile/noisy peer sends LF.
                let chunk = match reader.fill_buf().await {
                    Ok([]) => {
                        let _ = tx
                            .send(ReadFrame {
                                result: Err(HarnessError::Protocol("Pi stdout closed".into())),
                                _permit: None,
                            })
                            .await;
                        break;
                    }
                    Ok(chunk) => chunk,
                    Err(e) => {
                        let _ = tx
                            .send(ReadFrame {
                                result: Err(e.into()),
                                _permit: None,
                            })
                            .await;
                        break;
                    }
                };
                let count = chunk
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(chunk.len(), |i| i + 1);
                if bytes.len() + count > MAX_FRAME {
                    let _ = tx
                        .send(ReadFrame {
                            result: Err(HarnessError::Protocol(
                                "Pi RPC frame exceeds 32 MiB".into(),
                            )),
                            _permit: None,
                        })
                        .await;
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                reader.consume(count);
                if bytes.last() != Some(&b'\n') {
                    continue;
                }
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    let permit = read_budget
                        .clone()
                        .acquire_many_owned(bytes.len().min(QUEUE_BYTES) as u32)
                        .await
                        .expect("Pi read budget remains open");
                    let frame = serde_json::from_slice::<Value>(&bytes)
                        .map_err(|e| HarnessError::Protocol(format!("Invalid Pi RPC JSON: {e}")));
                    let failed = frame.is_err();
                    if tx
                        .send(ReadFrame {
                            result: frame,
                            _permit: Some(permit),
                        })
                        .await
                        .is_err()
                        || failed
                    {
                        break;
                    }
                }
                if bytes.capacity() > 64 * 1024 {
                    bytes = Vec::new();
                } else {
                    bytes.clear();
                }
            }
        });
        Self {
            client: Client {
                writer: writer_tx,
                budget,
                errors: client_errors,
                failed: Arc::new(AtomicBool::new(false)),
                next_id: Arc::new(AtomicU64::new(1)),
            },
            incoming: Incoming { receiver },
            reader,
            writer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn fragmented_utf8_and_responses_preserve_wire_order() {
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
        let id = transport
            .client
            .request(json!({"type":"get_state"}))
            .unwrap();
        let mut lines = BufReader::new(peer_read).lines();
        let request: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["id"], id);
        assert!(request.get("jsonrpc").is_none());
        let frames = [
            json!({"type":"message_update", "text":"ñ\u{2028}ok"}),
            json!({"type":"agent_settled"}),
            json!({"type":"response","id":id,"success":true}),
        ];
        for frame in &frames {
            for byte in format!("{frame}\r\n").bytes() {
                peer_write.write_all(&[byte]).await.unwrap();
            }
        }
        for expected in frames {
            assert_eq!(transport.incoming.recv().await.unwrap().unwrap(), expected);
        }
        peer_write.shutdown().await.unwrap();
        drop(peer_write);
        assert!(transport.incoming.recv().await.unwrap().is_err());
    }
    #[tokio::test]
    async fn malformed_json_is_a_protocol_failure() {
        let (host, mut peer) = tokio::io::duplex(64);
        let (read, write) = tokio::io::split(host);
        let mut transport = Transport::new(write, read);
        peer.write_all(b"not json\n").await.unwrap();
        assert!(
            transport
                .incoming
                .recv()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("Invalid Pi")
        );
    }

    #[tokio::test]
    async fn stalled_stdin_bounds_queue_and_preserves_accepted_order() {
        let (host, peer) = tokio::io::duplex(64);
        let (read, write) = tokio::io::split(host);
        let (peer_read, _peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
        // The writer has not been polled yet on this single-thread runtime.
        for i in 0..WRITE_QUEUE {
            transport.client.send(json!({"sequence":i})).unwrap();
        }
        let error = transport
            .client
            .send(json!({"sequence":"overflow"}))
            .unwrap_err();
        assert!(error.to_string().contains("queue full"));
        let mut lines = BufReader::new(peer_read).lines();
        for i in 0..WRITE_QUEUE {
            let line = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap(),
                json!({"sequence":i})
            );
        }
        // Even an ignored send failure reaches the actor so it can reap Pi.
        assert!(
            transport
                .incoming
                .recv()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("queue full")
        );
    }

    #[tokio::test]
    async fn one_large_write_leaves_control_headroom_until_transport_shutdown() {
        let (host, _peer) = tokio::io::duplex(64);
        let (read, write) = tokio::io::split(host);
        let transport = Transport::new(write, read);
        let client = transport.client.clone();
        client
            .send(json!({"message":"x".repeat(QUEUE_BYTES)}))
            .unwrap();
        assert_eq!(client.budget.available_permits(), CONTROL_HEADROOM);
        client.send(json!({"type":"get_state"})).unwrap();
        assert!(
            client
                .send(json!({"message":"y".repeat(QUEUE_BYTES)}))
                .unwrap_err()
                .to_string()
                .contains("256 KiB")
        );
        tokio::task::yield_now().await;
        // The budget includes the stalled in-flight frame, not just queued frames.
        assert!(client.budget.available_permits() > 0);
        assert!(client.budget.available_permits() < CONTROL_HEADROOM);
        drop(transport);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while client.budget.available_permits() != QUEUE_BYTES {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(client.budget.available_permits(), QUEUE_BYTES);
        assert!(
            client
                .send(json!({"type":"get_state"}))
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
    }

    #[tokio::test]
    async fn a_response_before_large_write_flush_can_queue_ordered_controls() {
        use futures::task::AtomicWaker;
        use std::{
            pin::Pin,
            task::{Context, Poll},
        };

        struct FlushGate {
            released: AtomicBool,
            waker: AtomicWaker,
        }
        struct HeldFlush<W> {
            writer: W,
            gate: Arc<FlushGate>,
        }
        impl<W: AsyncWrite + Unpin> AsyncWrite for HeldFlush<W> {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bytes: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Pin::new(&mut self.writer).poll_write(cx, bytes)
            }

            fn poll_flush(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                self.gate.waker.register(cx.waker());
                if !self.gate.released.load(Ordering::Acquire) {
                    return Poll::Pending;
                }
                Pin::new(&mut self.writer).poll_flush(cx)
            }

            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.writer).poll_shutdown(cx)
            }
        }

        let gate = Arc::new(FlushGate {
            released: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        });
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(
            HeldFlush {
                writer: write,
                gate: gate.clone(),
            },
            read,
        );
        let prompt_id = transport
            .client
            .request(json!({
                "type":"prompt", "message":"x".repeat(QUEUE_BYTES)
            }))
            .unwrap();
        let mut lines = BufReader::new(peer_read).lines();
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let prompt: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(prompt["id"], prompt_id);
        assert_eq!(prompt["message"].as_str().unwrap().len(), QUEUE_BYTES);
        assert_eq!(
            transport.client.budget.available_permits(),
            CONTROL_HEADROOM
        );

        // The peer received LF and responds while the writer still holds
        // the large request's permit awaiting flush. Runner sends get_state
        // at precisely this point; shutdown can also need clear_queue + abort.
        let response = json!({"type":"response", "id":prompt_id, "success":true});
        peer_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        let received =
            tokio::time::timeout(std::time::Duration::from_secs(2), transport.incoming.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        assert_eq!(received, response);
        let mut controls = Vec::new();
        for command in ["get_state", "clear_queue", "abort"] {
            let id = transport.client.request(json!({"type":command})).unwrap();
            controls.push((command, id));
        }
        assert!(!transport.client.failed.load(Ordering::Relaxed));
        assert!(
            transport
                .client
                .send(json!({
                    "type":"prompt", "message":"y".repeat(QUEUE_BYTES)
                }))
                .unwrap_err()
                .to_string()
                .contains("256 KiB")
        );

        gate.released.store(true, Ordering::Release);
        gate.waker.wake();
        for (command, id) in controls {
            let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(frame["type"], command);
            assert_eq!(frame["id"], id);
        }
    }

    #[tokio::test]
    async fn oversized_outgoing_frame_is_rejected_before_queueing() {
        let (host, _peer) = tokio::io::duplex(64);
        let (read, write) = tokio::io::split(host);
        let transport = Transport::new(write, read);
        let error = transport
            .client
            .send(json!({"message":"x".repeat(MAX_FRAME)}))
            .unwrap_err();
        assert!(error.to_string().contains("exceeds 32 MiB"));
        assert_eq!(transport.client.budget.available_permits(), QUEUE_BYTES);
        assert_eq!(transport.client.writer.capacity(), WRITE_QUEUE);
    }

    #[tokio::test]
    async fn large_incoming_frames_apply_byte_backpressure_and_keep_order() {
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (_peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
        let producer = tokio::spawn(async move {
            for sequence in 0..3 {
                let mut bytes = serde_json::to_vec(
                    &json!({"sequence":sequence,"text":"x".repeat(QUEUE_BYTES)}),
                )
                .unwrap();
                bytes.push(b'\n');
                peer_write.write_all(&bytes).await.unwrap();
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while transport.incoming.receiver.len() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // At most one large frame may occupy the queue's target byte budget.
        assert_eq!(transport.incoming.receiver.len(), 1);
        assert!(!producer.is_finished());
        for sequence in 0..3 {
            let frame =
                tokio::time::timeout(std::time::Duration::from_secs(2), transport.incoming.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            assert_eq!(frame["sequence"], sequence);
            assert_eq!(frame["text"].as_str().unwrap().len(), QUEUE_BYTES);
        }
        producer.await.unwrap();
    }
}

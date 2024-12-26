use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc},
};
use warp::Filter;
use reqwest_eventsource::{Event, EventSource};

use crate::error::McpError;
use super::{
    Transport, 
    TransportChannels,
    TransportCommand,
    TransportEvent,
    JsonRpcMessage,
};

use async_trait::async_trait;
use reqwest::Client;
use url::Url;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
};
use futures::TryStreamExt;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndpointEvent {
    endpoint: String,
}

pub struct SseTransport {
    host: String,
    port: u16,
    client_mode: bool,
    buffer_size: usize,
}

impl SseTransport {
    pub fn new_server(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: false,
            buffer_size,
        }
    }

    pub fn new_client(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: true,
            buffer_size,
        }
    }

    async fn run_server(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Create broadcast channel for SSE clients
        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<JsonRpcMessage>(100);
        let broadcast_tx = Arc::new(broadcast_tx);
        let broadcast_tx2 = Arc::clone(&broadcast_tx);

        // Client counter for unique IDs
        let client_counter = Arc::new(AtomicU64::new(0));
        let host_clone = host.clone();

        // SSE endpoint route
        let sse_route = warp::path("sse")
            .and(warp::get())
            .map(move || {
                let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
                let broadcast_rx = broadcast_tx.subscribe();
                let endpoint = format!("http://{}:{}/message/{}", host.clone(), port, client_id);

                warp::sse::reply(warp::sse::keep_alive()
                    .interval(Duration::from_secs(30))
                    .stream(async_stream::stream! {
                        yield Ok::<_, warp::Error>(warp::sse::Event::default()
                            .event("endpoint")
                            .data(endpoint));

                        let mut broadcast_rx = broadcast_rx;
                        while let Ok(msg) = broadcast_rx.recv().await {
                            yield Ok::<_, warp::Error>(warp::sse::Event::default()
                                .event("message")
                                .json_data(&msg)
                                .unwrap());
                        }
                    }))
            });

        // Message receiving route
        let message_route = warp::path!("message" / u64)
            .and(warp::post())
            .and(warp::body::json())
            .map(move |_client_id: u64, message: JsonRpcMessage| {
                let event_tx = event_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = event_tx.send(TransportEvent::Message(message)).await {
                        tracing::error!("Failed to forward message: {:?}", e);
                    }
                });
                warp::reply()
            });

        // Combine routes
        let routes = sse_route.or(message_route);

        // Message forwarding task
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        // Skip broadcasting debug log messages about SSE and internal operations
                        let should_skip = match &msg {
                            JsonRpcMessage::Notification(n) if n.method == "notifications/message" => {
                                if let Some(params) = &n.params {
                                    // Check the log message and logger
                                    let is_debug = params.get("level")
                                        .and_then(|l| l.as_str())
                                        .map_or(false, |l| l == "debug");
                                    
                                    let logger = params.get("logger")
                                        .and_then(|l| l.as_str())
                                        .unwrap_or("");

                                    let message = params.get("data")
                                        .and_then(|d| d.get("message"))
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("");

                                    is_debug && (
                                        logger.starts_with("hyper::") ||
                                        logger.starts_with("mcp_rs::transport") ||
                                        message.contains("Broadcasting SSE message") ||
                                        message.contains("Failed to broadcast message")
                                    )
                                } else {
                                    false
                                }
                            }
                            _ => false
                        };

                        if !should_skip {
                            tracing::debug!("Broadcasting SSE message: {:?}", msg);
                            if let Err(e) = broadcast_tx2.send(msg) {
                                tracing::error!("Failed to broadcast message: {:?}", e);
                            }
                        }
                    }
                    TransportCommand::Close => break,
                }
            }
        });

        // Start the server
        warp::serve(routes)
            .run((host_clone.parse::<IpAddr>().unwrap(), port))
            .await;
    }

    async fn run_client(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let client = reqwest::Client::new();
        let sse_url = format!("http://{}:{}/sse", host, port);

        tracing::debug!("Connecting to SSE endpoint: {}", sse_url);

        let rb = client.get(&sse_url);
        let mut sse = match EventSource::new(rb) {
            Ok(es) => es,
            Err(e) => {
                tracing::error!("Failed to create EventSource: {:?}", e);
                let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                return;
            }
        };

        // Wait for endpoint event
        let endpoint = match Self::wait_for_endpoint(&mut sse).await {
            Some(ep) => ep,
            None => {
                tracing::error!("Failed to receive endpoint");
                let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                return;
            }
        };

        tracing::debug!("Received message endpoint: {}", endpoint);

        // Message receiving task
        let event_tx2 = event_tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(event)) = sse.try_next().await {
                match event {
                    Event::Message(m) if m.event == "message" => {
                        match serde_json::from_str::<JsonRpcMessage>(&m.data) {
                            Ok(msg) => {
                                if let Err(e) = event_tx2.send(TransportEvent::Message(msg)).await {
                                    tracing::error!("Failed to forward SSE message: {:?}", e);
                                    break;
                                }
                            }
                            Err(e) => tracing::error!("Failed to parse SSE message: {:?}", e),
                        }
                    }
                    _ => continue,
                }
            }
        });

        // Message sending task
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    tracing::debug!("Sending message to {}: {:?}", endpoint, msg);
                    if let Err(e) = client.post(&endpoint).json(&msg).send().await {
                        tracing::error!("Failed to send message: {:?}", e);
                    }
                }
                TransportCommand::Close => break,
            }
        }

        let _ = event_tx.send(TransportEvent::Closed).await;
    }

    async fn wait_for_endpoint(sse: &mut EventSource) -> Option<String> {
        while let Ok(Some(event)) = sse.try_next().await {
            if let Event::Message(m) = event {
                if m.event == "endpoint" {
                    return serde_json::from_str::<EndpointEvent>(&m.data).ok().map(|e| e.endpoint);
                }
            }
        }
        None
    }
}

#[async_trait]
impl Transport for SseTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        if self.client_mode {
            tokio::spawn(Self::run_client(
                self.host.clone(),
                self.port,
                cmd_rx,
                event_tx,
            ));
        } else {
            tokio::spawn(Self::run_server(
                self.host.clone(),
                self.port,
                cmd_rx,
                event_tx,
            ));
        }

        let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));

        Ok(TransportChannels { cmd_tx, event_rx })
    }
}

use reqwest_eventsource::{Event, EventSource};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use axum::{
    extract::State,
    response::{sse::{Event as AxumSseEvent, Sse}, IntoResponse},
    routing::get,
    Router,
};
use tower_http::cors::CorsLayer;

use super::{JsonRpcMessage, Transport, TransportChannels, TransportCommand, TransportEvent};
use crate::error::McpError;

use async_trait::async_trait;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;

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
        let broadcast_tx_clone = Arc::clone(&broadcast_tx);

        // Client counter for unique IDs
        let client_counter = Arc::new(AtomicU64::new(0));
        let host_clone = host.clone();

        // Message forwarding task
        let message_task = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        // Skip broadcasting debug log messages
                        let should_skip = match &msg {
                            JsonRpcMessage::Notification(n)
                                if n.method == "notifications/message" =>
                            {
                                if let Some(params) = &n.params {
                                    let is_debug = params.get("level").and_then(|l| l.as_str())
                                        == Some("debug");

                                    let logger =
                                        params.get("logger").and_then(|l| l.as_str()).unwrap_or("");

                                    is_debug && logger.starts_with("mcp_rs::transport")
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        };

                        if !should_skip {
                            tracing::debug!("Broadcasting SSE message: {:?}", msg);
                            let receiver_count = broadcast_tx_clone.send(msg).unwrap_or(0);
                            if receiver_count == 0 {
                                tracing::debug!("No SSE clients connected to receive message");
                            }
                        }
                    }
                    TransportCommand::Close => {
                        tracing::info!("Closing SSE server message task");
                        break;
                    }
                }
            }
        });

        // SSE handler function
        async fn sse_handler(
            State((client_counter, broadcast_tx, host, port)): State<(
                Arc<AtomicU64>,
                Arc<tokio::sync::broadcast::Sender<JsonRpcMessage>>,
                String,
                u16,
            )>,
        ) -> impl IntoResponse {
            let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
            let broadcast_rx = broadcast_tx.subscribe();
            let endpoint = format!("http://{}:{}/message/{}", host, port, client_id);

            // Send initial endpoint information
            let stream = async_stream::stream! {
                // Send endpoint information first
                yield Ok::<_, Infallible>(
                    AxumSseEvent::default()
                        .event("endpoint")
                        .data(serde_json::to_string(&EndpointEvent { endpoint }).unwrap())
                );

                // Stream messages from broadcast channel
                let mut rx = broadcast_rx;
                while let Ok(msg) = rx.recv().await {
                    let json = serde_json::to_string(&msg).unwrap();
                    yield Ok::<_, Infallible>(
                        AxumSseEvent::default()
                            .event("message")
                            .data(json)
                    );
                }
            };

            Sse::new(stream).keep_alive(
                axum::response::sse::KeepAlive::new()
                    .interval(Duration::from_secs(30))
                    .text("ping")
            )
        }

        // Create the Axum app with routes
        let event_tx_clone = event_tx.clone();
        let app = Router::new()
            .route("/sse", get(sse_handler))
            .route("/message/:client_id", axum::routing::post(|| async { "OK" }))
            .layer(CorsLayer::permissive())
            .with_state((client_counter, broadcast_tx, host_clone, port))
            .with_state(event_tx_clone);

        // Parse the host address
        let host_addr: IpAddr = host
            .parse()
            .unwrap_or_else(|_| "127.0.0.1".parse().unwrap());
        let socket_addr = SocketAddr::from((host_addr, port));

        tracing::info!("Starting SSE server at http://{}:{}/sse", host, port);

        // Start the Axum server
        let listener = tokio::net::TcpListener::bind(socket_addr).await.unwrap();
        let server = axum::serve(listener, app);

        tokio::select! {
            _ = server => {
                tracing::info!("SSE server stopped");
            }
        }

        // Clean up
        message_task.abort();
        tracing::info!("SSE server shut down");
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
                let _ = event_tx
                    .send(TransportEvent::Error(McpError::ConnectionClosed))
                    .await;
                return;
            }
        };

        // Wait for endpoint event
        let endpoint = match Self::wait_for_endpoint(&mut sse).await {
            Some(ep) => ep,
            None => {
                tracing::error!("Failed to receive endpoint");
                let _ = event_tx
                    .send(TransportEvent::Error(McpError::ConnectionClosed))
                    .await;
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
                    return serde_json::from_str::<EndpointEvent>(&m.data)
                        .ok()
                        .map(|e| e.endpoint);
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


use protocol::JsonRpcNotification;

pub mod client;
pub mod error;
pub mod logging;
pub mod prompts;
pub mod protocol;
pub mod resource;
pub mod server;
pub mod tools;
pub mod transport;

#[derive(Debug, Clone)]
pub struct NotificationSender {
    pub tx: tokio::sync::mpsc::Sender<JsonRpcNotification>,
}

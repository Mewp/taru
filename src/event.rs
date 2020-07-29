use tokio::sync::broadcast::Sender;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub enum Event {
    Ping,
    Started(String),
    Finished(String, Option<i32>),
    UpdateConfig,
}

// This ignores any send errors, because they just mean that there were no receivers
pub fn send_message<T>(sender: &Sender<T>, message: T) {
    match sender.send(message) { _ => () };
}

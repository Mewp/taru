use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use std::sync::Arc;

#[derive(Clone)]
pub struct BroadcastChannel<T> {
    subscribers: Arc<Mutex<Vec<Option<Sender<T>>>>>,
    buffer: usize,
}

impl<T: Clone> BroadcastChannel<T> {
    pub fn new(buffer: usize) -> Self {
        BroadcastChannel {
            subscribers: Arc::new(Mutex::new(vec![])),
            buffer,
        }
    }

    pub async fn subscribe(&self) -> Receiver<T> {
        let mut subscribers = self.subscribers.lock().await;
        let (tx, rx) = channel(self.buffer);
        subscribers.push(Some(tx));
        rx
    }

    pub async fn send(&self, msg: T) {
        let mut subscribers = self.subscribers.lock().await;
        for subscriber in subscribers.iter_mut() {
            if let Some(sub) = subscriber {
                if let Err(_) = sub.send(msg.clone()).await {
                    *subscriber = None
                }
            }
        }
    }
}

use tokio::sync::broadcast::Sender;
use serde::Serialize;
use serde_json::json;
use bytes::{BytesMut, BufMut, Bytes};

#[derive(Debug, Serialize, Clone)]
pub enum Event {
    Ping,
    Started(String),
    Finished(String, Option<i32>),
    TaskData(String, String, String),
    UpdateConfig,
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Started(_) => "started",
            Self::Finished(_, _) => "finished",
            Self::TaskData(_, _, _) => "task_data",
            Self::UpdateConfig => "update_config"
        }
    }

    pub fn to_event(&self) -> Bytes {
        let mut data = BytesMut::new();
        data.put(&b"event: "[..]);
        data.put(self.name().as_bytes());
        data.put(&b"\ndata: "[..]);
        match self {
            Self::Started(task) => {
                data.put(serde_json::to_vec(&json!({"task": task})).unwrap().as_slice());
            }, Self::Finished(task, exit_code) => {
                data.put(serde_json::to_vec(&json!({"task": task, "exit_code": exit_code})).unwrap().as_slice());
            },
            _ => {}
        };
        data.put(&b"\n\n"[..]);
        Bytes::from(data)
    }
}

// This ignores any send errors, because they just mean that there were no receivers
pub fn send_message<T>(sender: &Sender<T>, message: T) {
    match sender.send(message) { _ => () };
}

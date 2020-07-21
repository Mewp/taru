use parking_lot::RwLock;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::broadcast;

use crate::cfg::Config;
use crate::task::{TaskState, TaskEvent, send_message};

#[derive(Clone)]
pub struct AppState {
    config_path: String,
    pub config: Config,
    pub tasks: HashMap<String, Arc<RwLock<TaskState>>>,
    pub events: tokio::sync::broadcast::Sender<(String, TaskEvent)>
}

impl AppState {
    pub fn new(config_path: impl Into<String>) -> Arc<RwLock<AppState>> {
        let config_path = config_path.into();
        let config = Config::read(&config_path); 
        let mut task_states = HashMap::new();
        let task_names: Vec<String> = config.tasks.keys().map(String::from).collect();
        for name in &task_names {
            task_states.insert(name.to_owned(), Arc::new(RwLock::new(TaskState::new(name))));
        }

        Arc::new(RwLock::new(AppState {
            config_path,
            config: config.clone(),
            tasks: task_states,
            events: broadcast::channel(16).0,
        }))
    }
}

pub fn reload_config(app_state: &Arc<RwLock<AppState>>) {
    let old_config = app_state.read().config.clone();
    let new_config = Config::read(&app_state.read().config_path);
    app_state.write().config = new_config.clone();

    for task in new_config.tasks.keys() {
        if !old_config.tasks.contains_key(task) {
            app_state.write().tasks.insert(task.to_owned(), Arc::new(RwLock::new(TaskState::new(task))));
        }
    }
    send_message(&app_state.read().events, ("".to_owned(), TaskEvent::UpdateConfig));
}

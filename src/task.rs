use os_pipe::pipe;
use mio::unix::SourceFd;
use mio::{Events, Poll, Token, Interest};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::process::{Command, Stdio, ExitStatus};
use tokio::sync::broadcast::Sender;
use parking_lot::RwLock;
use std::sync::Arc;
use serde::Serialize;
use bytes::{BytesMut, BufMut};
use tokio::process::Command as AsyncCommand;

const TOKEN_STDOUT: Token = Token(0);
const TOKEN_STDERR: Token = Token(1);
const BUF_SIZE: usize = 102400;

#[derive(Debug, Serialize, Clone)]
pub enum TaskEvent {
    Ping,
    Started,
    ExitStatus(Option<i32>),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    UpdateConfig,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    New,
    Running,
    Finished(Option<i32>)
}

impl TaskStatus {
    pub fn as_finished(&self) -> Option<i32> {
        if let TaskStatus::Finished(code) = self {
            return *code
        }
        None
    }
}

pub struct TaskState {
    pub name: String,
    pub status: TaskStatus,
    pub current_lines: usize,
    pub last_lines: usize,
    pub output: BytesMut,
    pub events: Sender<TaskEvent>
}

impl TaskState {
    pub fn new(name: impl Into<String>) -> Self {
        TaskState {
            name: name.into(),
            status: TaskStatus::New,
            current_lines: 0,
            last_lines: 0,
            output: BytesMut::new(),
            events: tokio::sync::broadcast::channel(16).0
        }
    }
}

// This ignores any send errors, because they just mean that there were no receivers
pub fn send_message<T>(sender: &Sender<T>, message: T) {
    match sender.send(message) { _ => () };
}

pub fn spawn_task(global_events: Sender<(String, TaskEvent)>, state: Arc<RwLock<TaskState>>, cmd: &Vec<String>, buffer: bool) {
    let (mut reader_out, writer_out) = pipe().unwrap();
    let (mut reader_err, writer_err) = pipe().unwrap();
    let mut cmd_base = Command::new("systemd-run");
    cmd_base.args(&["--user", "--quiet", "--scope", "--collect", &format!("--unit=taru-task-{}", state.read().name)]);
    cmd_base.args(cmd);
    cmd_base.stdin(Stdio::null());
    cmd_base.stdout(writer_out);
    cmd_base.stderr(writer_err);
    let mut poll = Poll::new().unwrap();
    poll.registry().register(&mut SourceFd(&reader_out.as_raw_fd()), TOKEN_STDOUT, Interest::READABLE).unwrap();
    poll.registry().register(&mut SourceFd(&reader_err.as_raw_fd()), TOKEN_STDERR, Interest::READABLE).unwrap();
    let mut events = Events::with_capacity(16);
    let mut cmd = cmd_base.spawn().unwrap();
    let mut buf = [0u8; BUF_SIZE];
    drop(cmd_base);  // crucial to drop writing pipes
    let task_id = state.read().name.clone();
    let task_events = state.read().events.clone();
    std::thread::Builder::new().name(format!("task {}", task_id)).spawn(move || {
        state.write().status = TaskStatus::Running;
        send_message(&global_events, (task_id.clone(), TaskEvent::Started));
        let mut closed = 0;
        while closed < 2 {
            poll.poll(&mut events, None).unwrap();
            for event in events.iter() {
                let token = event.token();
                if event.is_read_closed() {
                    closed += 1;
                    if event.is_readable() {
                        let mut buf: Vec<u8> = Vec::with_capacity(BUF_SIZE);
                        if token == TOKEN_STDOUT {
                            reader_out.read_to_end(&mut buf).unwrap();
                            send_message(&task_events, TaskEvent::Stdout(buf[..].into()));
                        } else {
                            reader_err.read_to_end(&mut buf).unwrap();
                            send_message(&task_events, TaskEvent::Stderr(buf[..].into()));
                        }
                        if buffer {
                            state.write().output.put(&buf[..]);
                        }
                    };
                } else if event.is_readable() {
                    let res = if token == TOKEN_STDOUT {
                        let res = reader_out.read(&mut buf).unwrap();
                        send_message(&task_events, TaskEvent::Stdout(buf[0..res].to_owned()));
                        res
                    } else {
                        let res = reader_err.read(&mut buf).unwrap();
                        send_message(&task_events, TaskEvent::Stderr(buf[0..res].to_owned()));
                        res
                    };

                    if buffer {
                        state.write().output.put(&buf[0..res]);
                    }
                }
            }
        }
        let code = cmd.wait().expect("wait() failed").code();
        state.write().status = TaskStatus::Finished(code);
        send_message(&task_events, TaskEvent::ExitStatus(code));
        send_message(&global_events, (task_id.clone(), TaskEvent::ExitStatus(code)));
    }).unwrap();
}

// This can be async, because it deosn't stream the output
pub async fn stop_task(state: Arc<RwLock<TaskState>>) -> tokio::io::Result<ExitStatus> {
    let ref name = state.read().name;
    AsyncCommand::new("systemctl")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args(&["--user", "stop", &format!("taru-task-{}.scope", name)])
        .status().await
}

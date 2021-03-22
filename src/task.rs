use std::collections::HashMap;
use os_pipe::pipe;
use mio::unix::SourceFd;
use mio::{Events, Poll, Token, Interest};
use std::io::{Read, ErrorKind};
use std::os::unix::io::{RawFd, AsRawFd};
use std::process::{Command, Stdio, ExitStatus};
use tokio::sync::broadcast::Sender;
use parking_lot::RwLock;
use std::sync::Arc;
use serde::Serialize;
use bytes::{BytesMut, BufMut};
use tokio::process::Command as AsyncCommand;

use crate::event::{Event, send_message};
use crate::broadcast::BroadcastChannel;
use libc::{fcntl, F_GETFL, F_SETFL, O_NONBLOCK};

const TOKEN_STDOUT: Token = Token(0);
const TOKEN_STDERR: Token = Token(1);
const BUF_SIZE: usize = 10240;

#[derive(Debug, Serialize, Clone)]
pub enum TaskOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Finished(Option<i32>)
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

    pub fn is_finished(&self) -> bool {
        match self {
            TaskStatus::Finished(_) => true,
            _ => false
        }
    }
}

pub struct TaskState {
    pub name: String,
    pub status: TaskStatus,
    pub current_lines: usize,
    pub last_lines: usize,
    pub output: BytesMut,
    pub events: BroadcastChannel<TaskOutput>,
    pub data: HashMap<String, String>
}

impl TaskState {
    pub fn new(name: impl Into<String>) -> Self {
        TaskState {
            name: name.into(),
            status: TaskStatus::New,
            current_lines: 0,
            last_lines: 0,
            output: BytesMut::new(),
            events: BroadcastChannel::new(16),
            data: HashMap::new()
        }
    }
}

fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    unsafe {
        let flags = fcntl(fd, F_GETFL, 0);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    }

    Ok(())
}

pub async fn spawn_task(global_events: Sender<Event>, task: Arc<RwLock<TaskState>>, cmdline: &Vec<String>, buffer: bool) {
    // This is a mio-based implementation of running a process asynchronously and capturing its
    // stdout and stderr. Mio is used here directly because in order to preserve the order of
    // wakeup events, we need to use one Poll for both streams.
    let (mut reader_out, writer_out) = pipe().unwrap();
    let (mut reader_err, writer_err) = pipe().unwrap();
    let mut cmd = Command::new("systemd-run");
    cmd.args(&["--user", "--quiet", "--scope", "--collect", &format!("--unit=taru-task-{}", task.read().name)]);
    cmd.args(cmdline);
    cmd.stdin(Stdio::null());
    cmd.stdout(writer_out);
    cmd.stderr(writer_err);
    let mut poll = Poll::new().unwrap();
    set_nonblocking(reader_out.as_raw_fd()).unwrap();
    set_nonblocking(reader_err.as_raw_fd()).unwrap();
    poll.registry().register(&mut SourceFd(&reader_out.as_raw_fd()), TOKEN_STDOUT, Interest::READABLE).unwrap();
    poll.registry().register(&mut SourceFd(&reader_err.as_raw_fd()), TOKEN_STDERR, Interest::READABLE).unwrap();
    let mut events = Events::with_capacity(16);
    let mut child = cmd.spawn().unwrap();
    let mut buf = Box::new([0u8; BUF_SIZE]);
    drop(cmd);  // crucial to drop writing pipes
    let task_name = task.read().name.clone();
    let task_events = task.read().events.clone();
    std::thread::Builder::new().name(format!("task {}", task_name)).spawn(move || {
        // A new thread requires a new tokio runtime,
        // and a new thread is required because mio is blocking.
        // Now if I could have just added a new scheduler to tokio, it would have been easier.
        let mut runtime = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        runtime.block_on(async move {
            task.write().status = TaskStatus::Running;
            send_message(&global_events, Event::Started(task_name.clone()));
            let mut closed = 0u8;
            while closed < 2 {
                poll.poll(&mut events, None).unwrap();
                for event in events.iter() {
                    let token = event.token();
                    if event.is_read_closed() {
                        closed += 1;
                    }
                    if event.is_readable() {
                        if token == TOKEN_STDOUT {
                            loop {
                                let res = match reader_out.read(&mut buf[..]) {
                                    Ok(0) => break,
                                    Ok(res) => res,
                                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                    Err(e) => panic!(e)
                                };
                                task_events.send(TaskOutput::Stdout(buf[0..res].to_owned())).await;

                                if buffer {
                                    task.write().output.put(&buf[0..res]);
                                }
                            }
                        } else {
                            loop {
                                let res = match reader_err.read(&mut buf[..]) {
                                    Ok(0) => break,
                                    Ok(res) => res,
                                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                    Err(e) => panic!(e)
                                };
                                task_events.send(TaskOutput::Stderr(buf[0..res].to_owned())).await;

                                if buffer {
                                    task.write().output.put(&buf[0..res]);
                                }
                            }
                        };
                    }
                }
            }
            let code = child.wait().expect("wait() failed").code();
            task.write().status = TaskStatus::Finished(code);
            task_events.send(TaskOutput::Finished(code)).await;
            send_message(&global_events, Event::Finished(task_name.clone(), code));
        });
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

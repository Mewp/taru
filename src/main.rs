use actix_web::{
    web, App, HttpRequest, HttpServer, HttpResponse,
    get, post, Scope, dev::{ServiceRequest, ServiceResponse},
    error::Error as ActixError
};
use actix_files::Files;
use listenfd::ListenFd;
use futures::stream::{self, StreamExt};
use futures::{future, FutureExt, Future};
use bytes::{Bytes, BytesMut};
use std::convert::Infallible;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use parking_lot::RwLock;
use std::sync::Arc;
use actix_service::Service;
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::{self, Duration};
use tokio::select;
use serde::Serialize;
use serde_json;
use libc::geteuid;

mod cfg;
mod event;
mod broadcast;
mod task;
mod app_state;

use app_state::AppState;
use task::TaskOutput;
use event::{Event, send_message};

#[derive(Debug, Serialize)]
struct TaskData<'a> {
    name: &'a str,
    meta: &'a serde_json::Value,
    state: &'static str,
    exit_code: Option<i32>,
    can_run: bool,
    can_view_output: bool
}

fn get_view_status_tasks(req: &HttpRequest) -> HashSet<String> {
    let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
    let login = req.headers().get("x-user").map(|h| h.to_str().unwrap());
    match login {
        Some(login) => data.read().config.users.get(login).unwrap().can_view_status.iter().map(String::from).collect(),
        None => data.read().config.tasks.keys().map(String::from).collect()
    }
}

fn get_view_output_tasks(req: &HttpRequest) -> HashSet<String> {
    let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
    let login = req.headers().get("x-user").map(|h| h.to_str().unwrap());
    match login {
        Some(login) => data.read().config.users.get(login).unwrap().can_view_output.iter().map(String::from).collect(),
        None => data.read().config.tasks.keys().map(String::from).collect()
    }
}

fn get_run_tasks(req: &HttpRequest) -> HashSet<String> {
    let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
    let login = req.headers().get("x-user").map(|h| h.to_str().unwrap());
    match login {
        Some(login) => data.read().config.users.get(login).unwrap().can_run.iter().map(String::from).collect(),
        None => data.read().config.tasks.keys().map(String::from).collect()
    }
}

fn can_run_task(req: &HttpRequest) -> bool {
    let task = req.match_info().get("task").unwrap();
    get_run_tasks(req).iter().find(|t| *t == &task).is_some()
}

fn can_view_output(req: &HttpRequest) -> bool {
    let task = req.match_info().get("task").unwrap();
    get_view_output_tasks(req).iter().find(|t| *t == &task).is_some()
}

#[get("/tasks")]
async fn tasks(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>) -> HttpResponse {
    let data = data.read();
    let can_run: HashSet<_> = get_run_tasks(&req);
    let can_view_output: HashSet<_> = get_view_output_tasks(&req);
    let tasks = get_view_status_tasks(&req);
    HttpResponse::Ok().json(
        tasks.iter().map(|name| {
            let task = data.tasks.get(name).unwrap().read();
            (name, TaskData {
                name: name,
                meta: &data.config.tasks[name].meta,
                state: match task.status {
                    task::TaskStatus::New => "new",
                    task::TaskStatus::Running => "running",
                    task::TaskStatus::Finished(_) => "finished"
                },
                exit_code: task.status.as_finished(),
                can_run: can_run.contains(name),
                can_view_output: can_view_output.contains(name)
            })
        }).collect::<HashMap<_, _>>()
    )
}

async fn run_task(req: &HttpRequest, data: &web::Data<Arc<RwLock<AppState>>>, params: &web::Path<(String,)>) -> Result<(), HttpResponse> {
    if !can_run_task(&req) {
        return Err(HttpResponse::NotFound().finish())
    }
    let events = data.read().events.clone();
    let data = data.read();
    let ref task = data.config.tasks.get(&params.0).unwrap();
    let cmdline = task.command.clone();
    let state = data.tasks.get(&params.0).unwrap().clone();
    if state.read().status == task::TaskStatus::Running {
        return Err(HttpResponse::Conflict().body("The task is already running. Refusing to run two instances in parallel."));
    }
    state.write().output = BytesMut::new();
    let is_buffered = task.buffered;
    tokio::spawn(async move { task::spawn_task(events, state, &cmdline, is_buffered).await });

    Ok(())
}

async fn stream_task(req: &HttpRequest, data: &web::Data<Arc<RwLock<AppState>>>, params: &web::Path<(String,)>, print_output: bool) -> Result<HttpResponse, HttpResponse> {
    if !can_view_output(&req) {
        return Err(HttpResponse::NotFound().finish())
    }

    let data = data.read();
    let task = data.tasks.get(&params.0).unwrap().read();
    let receiver = task.events.subscribe().await;

    let stream = receiver.take_while(|event| future::ready(
        match event {
            TaskOutput::Finished(_) => false,
            _ => true
        }
    )).filter_map(|event| async move {
        match event {
            TaskOutput::Stdout(data) => Some(Ok::<_, Infallible>(Bytes::from(data))),
            TaskOutput::Stderr(data) => Some(Ok::<_, Infallible>(Bytes::from(data))),
            _ => None
        }
    });

    let mut resp = HttpResponse::Ok();
    resp.header("content-type", "text/plain; charset=utf-8");
    resp.header("x-content-type-options", "nosniff");
    for (name, value) in &data.config.tasks.get(&params.0).unwrap().headers {
        resp.set_header(name, value.as_str());
    }

    if print_output {
        let body = stream::once(future::ready(
            Ok::<_, Infallible>(Bytes::from(task.output.clone()))
        )).chain(stream);

        Ok(resp.streaming(body))
    } else {
        Ok(resp.streaming(stream))
    }
}

#[post("/task/{task}/output")]
async fn task_run_stream(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>) -> HttpResponse {
    let stream = match stream_task(&req, &data, &params, false).await {
        Ok(stream) => stream,
        Err(response) => return response
    };

    if let Err(response) = run_task(&req, &data, &params).await {
        return response
    }

    stream
}

#[post("/task/{task}")]
async fn task_run(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>) -> actix_web::Result<HttpResponse> {
    if let Err(response) = run_task(&req, &data, &params).await {
        return Ok(response)
    }

    Ok(HttpResponse::Ok().body("Ok"))
}

#[post("/task/{task}/stop")]
async fn task_stop(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>) -> HttpResponse {
    if !can_run_task(&req) {
        return HttpResponse::NotFound().finish()
    }

    let state = data.read().tasks.get(&params.0).unwrap().clone();
    if state.read().status == task::TaskStatus::Running {
        if let Err(e) = task::stop_task(state).await {
            return HttpResponse::InternalServerError().body(format!("Stopping task failed: {}", e));
        }
    }

    HttpResponse::Ok().body("Ok")
}

#[get("/task/{task}/output")]
async fn task_stream(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>) -> HttpResponse {
    if !can_view_output(&req) {
        return HttpResponse::NotFound().finish()
    }

    let data_read = data.read();
    let task = data_read.tasks.get(&params.0).unwrap().read();
    if task.status != task::TaskStatus::Running {
        let mut resp = HttpResponse::Ok();
        resp.header("content-type", "text/plain; charset=utf-8");
        resp.header("x-content-type-options", "nosniff");
        for (name, value) in &data_read.config.tasks.get(&params.0).unwrap().headers {
            resp.set_header(name, value.as_str());
        }
        
        return resp.body(task.output.clone())
    }
    stream_task(&req, &data, &params, true).await.unwrap_or_else(|e| e)
}

#[get("/events")]
async fn sse(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>) -> HttpResponse {
    let receiver = data.read().events.subscribe();
    let task_access: HashSet<_> = get_view_status_tasks(&req);

    let first_ping = stream::once(future::ready(Ok::<_, Infallible>(Event::Ping.to_event())));
    let stream = receiver.into_stream().scan(task_access, |task_access, result|
        match result {
            Ok(event) => {
                match &event {
                    Event::Started(name)
                    | Event::Finished(name, _) => if !task_access.contains(name) {
                        return future::ready(None)
                    },
                    _ => {}
                }
                future::ready(Some(Ok::<_, Infallible>(event.to_event())))
            }
            Err(_) => future::ready(None)
        }
    );
    let body = first_ping.chain(stream);

    HttpResponse::Ok().header("Content-Type", "text/event-stream").streaming(body)
}

fn forbidden(req: ServiceRequest) -> Pin<Box<dyn Future<Output = Result<ServiceResponse, ActixError>>>> {
    future::ready(
        Ok(req.into_response(
            HttpResponse::Forbidden().finish()
        ))
    ).boxed_local()
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    // systemctl won't know how to connect to the user instance unless this env var is set
    std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { geteuid() }));
    let mut listenfd = ListenFd::from_env();
    let data = AppState::new(std::env::args().collect::<Vec<_>>().get(1).expect("The first argument must be a path to the config file."));
    let signal_data = data.clone();

    let mut server = HttpServer::new(move ||
        App::new().data(data.clone())
            .wrap_fn(|req, srv| {
                if req.app_data::<Arc<RwLock<AppState>>>().unwrap().read().config.users.len() == 0 {
                    // Disable authorization if there are no users defined
                    return srv.call(req)
                }

                // Otherwise, reject any unauthorized users.
                if let Some(Ok(login)) = req.headers().get("x-user").map(|h| h.to_str()) {
                    if !req.app_data::<Arc<RwLock<AppState>>>().unwrap().read().config.users.contains_key(login) {
                        // We don't know this user
                        return forbidden(req)
                    }
                    srv.call(req)
                } else {
                    // No X-User header has been set, or it was somehow incorrect.
                    forbidden(req)
                }
            })
            .service(
                Scope::new("/api/v1").service(sse).service(tasks)
                    .service(task_run).service(task_stream).service(task_run_stream).service(task_stop)
            )
            .service(Files::new("/", "public").index_file("index.html"))
    );

    tokio::spawn(async move {
        let mut sighup = signal(SignalKind::hangup()).unwrap();
        let data = signal_data;
        loop {
            let heartbeat = data.read().config.heartbeat.clone();
            if let Some(heartbeat) = heartbeat {
                select!(
                    _ = sighup.recv() => { app_state::reload_config(&data); }
                    _ = time::delay_for(Duration::from_secs(heartbeat)) => {
                        send_message(&data.read().events, Event::Ping);
                    }
                )
            } else {
                sighup.recv().await;
                app_state::reload_config(&data);
            }
        }
    });

    server = if let Some(l) = listenfd.take_tcp_listener(0).unwrap() {
        server.listen(l)?
    } else {
        server.bind("0.0.0.0:3000")?
    };

    server.run().await
}

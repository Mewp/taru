use actix_web::{
    web, App, HttpRequest, HttpServer, HttpResponse,
    get, post, Scope, dev::{ServiceRequest, ServiceResponse},
    error::Error as ActixError, FromRequest
};
use serde::Deserialize;
use http::StatusCode;
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
use paste::paste;

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
    data: HashMap<String, String>,
    arguments: &'a Vec<cfg::Argument>,
    argument_values: HashMap<String, String>,
    state: &'static str,
    exit_code: Option<i32>,
    can_run: bool,
    can_view_output: bool
}

macro_rules! generate_perm_checks {
    ($name:ident) => {
        paste! {
            fn [<get_ $name _tasks>](req: &HttpRequest) -> HashSet<String> {
                let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
                let login = req.headers().get("x-user").map(|h| h.to_str().unwrap());
                match login {
                    Some(login) => data.read().config.users.get(login).unwrap().[<can_ $name>].iter().map(String::from).collect(),
                    None => data.read().config.tasks.keys().map(String::from).collect()
                }
            }

            #[allow(dead_code)]
            fn [<can_ $name>](req: &HttpRequest) -> bool {
                let task = req.match_info().get("task").unwrap();
                [<get_ $name _tasks>](req).iter().find(|t| *t == &task).is_some()
            }
        }
    }
}

generate_perm_checks!(view_status);
generate_perm_checks!(view_output);
generate_perm_checks!(run);
generate_perm_checks!(change_data);

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
                data: task.data.clone(),
                state: match task.status {
                    task::TaskStatus::New => "new",
                    task::TaskStatus::Running => "running",
                    task::TaskStatus::Finished(_) => "finished"
                },
                arguments: &data.config.tasks[name].arguments,
                argument_values: task.arguments.clone(),
                exit_code: task.status.as_finished(),
                can_run: can_run.contains(name),
                can_view_output: can_view_output.contains(name)
            })
        }).collect::<HashMap<_, _>>()
    )
}

async fn run_task(req: &HttpRequest, data: &web::Data<Arc<RwLock<AppState>>>, params: &web::Path<(String,)>) -> Result<(), HttpResponse> {
    if !can_run(&req) {
        return Err(HttpResponse::NotFound().finish())
    }
    let events = data.read().events.clone();
    let data = data.read();
    let ref task = data.config.tasks.get(&params.0).unwrap();
    let state = data.tasks.get(&params.0).unwrap().clone();
    if state.read().status == task::TaskStatus::Running {
        return Err(HttpResponse::Conflict().body("The task is already running. Refusing to run two instances in parallel."));
    }
    let mut args = HashMap::new();
    let params = web::Query::<HashMap<String, String>>::extract(req).await.unwrap().into_inner();
    let post = if let Ok(payload) = web::Form::<HashMap<String, String>>::extract(req).await {
        payload.into_inner()
    } else { HashMap::new() };

    for arg in &task.arguments {
        if let Some(value) = params.get(&arg.name).or(post.get(&arg.name)) {
            if arg.datatype == cfg::ArgumentType::Int && value.parse::<i32>().is_err() {
                return Err(HttpResponse::BadRequest().body(format!("Argument {} has to be a number, but is `{}` instead.", arg.name, value)));
            }
            if arg.datatype == cfg::ArgumentType::Enum {
                if let Some(ref enum_source) = arg.enum_source {
                    let state = data.tasks.get(enum_source).unwrap().clone();
                    if !state.read().status.is_finished() {
                        return Err(HttpResponse::BadRequest().body(format!("Data source of argument {} is not ready yet.", arg.name)));
                    }
                    if value.len() == 0 {
                        return Err(HttpResponse::BadRequest().body(format!("Empty value for argument {}", arg.name)));
                    }
                    let output: &[u8] = &*state.read().output;
                    if output.split(|byte| *byte == b'\n').find(|val| *val == value.as_bytes()).is_none() {
                        return Err(HttpResponse::BadRequest().body(format!("Argument {} has an invalid value.", arg.name)));
                    }
                } else {
                    return Err(HttpResponse::InternalServerError().body(format!("Argument {} is an enum without a data source, please fix its configuration.", arg.name)));
                }
            }
            args.insert(arg.name.clone(), value.clone());
            state.write().arguments.insert(arg.name.clone(), value.clone());
        } else {
            return Err(HttpResponse::BadRequest().body(format!("Missing argument {}", arg.name)));
        }
    }

    let cmdline = task.command.iter().map(|segment|
        if segment.starts_with("$") {
            if args.contains_key(&segment[1..]) {
                args.get(&segment[1..]).unwrap().clone()
            } else if segment == "$taru_user" {
                if let Some(Ok(login)) = req.headers().get("x-user").map(|h| h.to_str()) {
                    login.to_owned()
                } else {
                    String::new()
                }
            } else {
                segment.clone()
            }
        } else {
            segment.clone()
        }
    ).collect();

    state.write().output = BytesMut::new();
    let is_buffered = task.buffered;
    if task::spawn_task(events, state, &cmdline, is_buffered).await.is_err() {
        return Err(HttpResponse::Conflict().body("The task is already running. Refusing to run two instances in parallel."));
    }

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

    if print_output && data.config.tasks.get(&params.0).unwrap().buffered {
        let body = stream::once(future::ready(
            Ok::<_, Infallible>(Bytes::from(task.output.clone()))
        )).chain(stream);

        Ok(resp.streaming(body))
    } else {
        Ok(resp.streaming(stream))
    }
}

#[derive(Deserialize)]
struct WaitForStatus {
    #[serde(default)]
    check: bool
}

async fn wait_for_status(req: &HttpRequest, data: &web::Data<Arc<RwLock<AppState>>>, params: &web::Path<(String,)>, query: &web::Query<WaitForStatus>) -> Result<HttpResponse, HttpResponse> {
    if !can_view_output(&req) {
        return Err(HttpResponse::NotFound().finish())
    }

    let mut receiver = {
        let data = data.read();
        let task = data.tasks.get(&params.0).unwrap().read();
        task.events.subscribe().await
    };

    while let Some(msg) = receiver.next().await {
        println!("{:?}", msg);
        if let TaskOutput::Finished(code) = msg {
            let code = code.unwrap_or(-1);
            let mut resp = HttpResponse::build(StatusCode::from_u16(520).unwrap());
            resp.header("content-type", "text/plain; charset=utf-8");
            resp.header("x-content-type-options", "nosniff");
            if !query.check || code == 0 {
                resp.status(StatusCode::OK);
            }
            return Ok(resp.body(format!("{}", code)));
        }
    }

    return Ok(HttpResponse::InternalServerError().body("The task has ended, but hasn't notified me. This is a bug."));
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

#[get("/task/{task}/status")]
async fn task_wait(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>, query: web::Query<WaitForStatus>) -> Result<HttpResponse, HttpResponse> {
    if !can_view_status(&req) {
        return Ok(HttpResponse::NotFound().finish())
    }

    let status = data.read().tasks.get(&params.0).unwrap().read().status.clone();
    match status {
        task::TaskStatus::New => Ok(HttpResponse::NoContent().finish()),
        task::TaskStatus::Running => wait_for_status(&req, &data, &params, &query).await,
        task::TaskStatus::Finished(code) => {
            let code = code.unwrap_or(-1);
            let mut resp = HttpResponse::build(StatusCode::from_u16(520).unwrap());
            resp.header("content-type", "text/plain; charset=utf-8");
            resp.header("x-content-type-options", "nosniff");
            if !query.check || code == 0 {
                resp.status(StatusCode::OK);
            }
            return Ok(resp.body(format!("{}", code)));
        }
    }
}

#[post("/task/{task}/status")]
async fn task_run_wait(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>, query: web::Query<WaitForStatus>) -> Result<HttpResponse, HttpResponse> {
    if !can_view_status(&req) {
        return Ok(HttpResponse::NotFound().finish())
    }


    if let Err(response) = run_task(&req, &data, &params).await {
        return Ok(response)
    }

    wait_for_status(&req, &data, &params, &query).await
}

#[post("/task/{task}/stop")]
async fn task_stop(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String,)>) -> HttpResponse {
    if !can_run(&req) {
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

#[post("/task/{task}/data/{name}")]
async fn task_change_data(req: HttpRequest, mut body: web::Payload, data: web::Data<Arc<RwLock<AppState>>>, params: web::Path<(String, String)>) -> actix_web::Result<HttpResponse> {
    if !can_change_data(&req) {
        return Ok(HttpResponse::NotFound().finish())
    }

    let mut bytes = web::BytesMut::new();
    while let Some(item) = body.next().await {
        bytes.extend_from_slice(&item?);
    }

    let data_read = data.read();
    let mut task = data_read.tasks.get(&params.0).unwrap().write();
    let value = String::from_utf8_lossy(&bytes).into_owned();
    task.data.insert(params.1.clone(), value.clone());
    send_message(&data_read.events, Event::TaskData(task.name.clone(), params.1.clone(), value));

    Ok(HttpResponse::Ok().body("Ok"))
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
                    Event::Started(name, _)
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
                    .service(task_change_data).service(task_run_wait).service(task_wait)
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

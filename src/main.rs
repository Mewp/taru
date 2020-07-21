use actix_web::{web, App, HttpRequest, HttpServer, HttpResponse, get, post, Scope};
use actix_web::http::{header, HeaderName, HeaderValue};
use actix_files::Files;
use listenfd::ListenFd;
use futures::stream::{self, StreamExt};
use futures::{future};
use bytes::{Bytes, BytesMut, BufMut};
use std::convert::Infallible;
use std::collections::{HashMap, HashSet};
use parking_lot::RwLock;
use std::sync::Arc;
use actix_service::Service;
use actix_session::{CookieSession, UserSession};
use tokio::signal::unix::{signal, SignalKind};
use serde::Serialize;
use serde_json;

mod cfg;
mod task;
mod auth;
mod app_state;

use app_state::AppState;
use task::TaskEvent;

#[derive(Debug, Serialize)]
struct TaskData<'a> {
    name: &'a str,
    meta: &'a serde_json::Value,
    state: &'static str,
    exit_code: Option<i32>,
    can_run: bool,
    can_view_output: bool
}

fn can_run_task(req: &HttpRequest) -> bool {
    let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
    let login = req.headers().get("x-user").unwrap().to_str().unwrap();
    let task = req.match_info().get("task").unwrap();
    data.read().config.users.get(login).unwrap().can_run.iter().find(|t| *t == task).is_some()
}

fn can_view_output(req: &HttpRequest) -> bool {
    let data: &web::Data<Arc<RwLock<AppState>>> = req.app_data().unwrap();
    let login = req.headers().get("x-user").unwrap().to_str().unwrap();
    let task = req.match_info().get("task").unwrap();
    data.read().config.users.get(login).unwrap().can_view_output.iter().find(|t| *t == task).is_some()
}

#[get("/tasks")]
async fn tasks(req: HttpRequest, data: web::Data<Arc<RwLock<AppState>>>) -> HttpResponse {
    let login = req.headers().get("x-user").unwrap().to_str().unwrap();
    let data = data.read();
    let can_run: HashSet<_> = data.config.users.get(login).unwrap().can_run.iter().collect();
    let can_view_output: HashSet<_> = data.config.users.get(login).unwrap().can_view_output.iter().collect();
    HttpResponse::Ok().json(
        data.config.users.get(login).unwrap().can_view_status.iter().map(|name| {
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
    let ref cmdline = task.command;
    let state = data.tasks.get(&params.0).unwrap().clone();
    if state.read().status == task::TaskStatus::Running {
        return Err(HttpResponse::Conflict().body("The task is already running. Refusing to run two instances in parallel."));
    }
    state.write().output = BytesMut::new();
    task::spawn_task(events, state, cmdline, task.buffered);

    Ok(())
}

async fn stream_task(req: &HttpRequest, data: &web::Data<Arc<RwLock<AppState>>>, params: &web::Path<(String,)>, print_output: bool) -> Result<HttpResponse, HttpResponse> {
    if !can_view_output(&req) {
        return Err(HttpResponse::NotFound().finish())
    }

    let data = data.read();
    let task = data.tasks.get(&params.0).unwrap().read();
    let receiver = task.events.subscribe();

    let stream = receiver.into_stream().take_while(|result| future::ready(
        match result {
            Ok(TaskEvent::ExitStatus(_)) => false,
            _ => true
        }
    )).filter_map(|result| async move {
        match result {
            Ok(event) => {
                match event {
                    TaskEvent::Stdout(data) => Some(Ok::<_, Infallible>(Bytes::from(data))),
                    TaskEvent::Stderr(data) => Some(Ok::<_, Infallible>(Bytes::from(data))),
                    _ => None
                }
            }
            Err(_) => None
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
    let login = req.headers().get("x-user").unwrap().to_str().unwrap();
    let task_access: HashSet<_> = data.read().config.users.get(login).unwrap().can_view_status.iter().map(|task|
        task.clone()
    ).collect();

    let body = receiver.into_stream().scan(task_access, |task_access, result|
        match result {
            Ok(event) => {
                if !task_access.contains(&event.0) {
                    return future::ready(None)
                }
                let mut data = BytesMut::new();
                data.put(&b"data: "[..]);
                data.put(serde_json::to_vec(&event).unwrap().as_slice());
                data.put(&b"\n\n"[..]);
                future::ready(Some(Ok::<_, Infallible>(Bytes::from(data))))
            }
            Err(_) => future::ready(None)
        }
    );

    HttpResponse::Ok().header("Content-Type", "text/event-stream").streaming(body)
}

// Import a geteuid syscall, so that we can get the effective user id without depending on a huge
// library. This program won't run on anything other than linux anyway, so this should be safe.
#[link(name="c")]
extern "C" {
    fn geteuid() -> u32;
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    // systemctl won't know how to connect to the user instance unless this env var is set
    std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { geteuid() }));
    let mut listenfd = ListenFd::from_env();
    let data = AppState::new("taru.yml");
    let signal_data = data.clone();
    let config = data.read().config.clone();

    let mut server = HttpServer::new(move ||
        App::new().data(data.clone())
            .wrap_fn(|mut req, srv| {
                if req.uri().path().starts_with("/auth") {
                    return srv.call(req)
                }
                let logged_in: bool = req.get_session().get::<String>("login").unwrap().is_some();
                if !logged_in {
                    return Box::pin(async {
                        Ok(req.into_response(
                            HttpResponse::Found().header(header::LOCATION, "/auth/gitlab/login".to_string()).finish()
                        ))
                    })
                }
                let login: String = req.get_session().get::<String>("login").unwrap().unwrap();
                if !req.app_data::<Arc<RwLock<AppState>>>().unwrap().read().config.users.contains_key(&login) {
                    return Box::pin(async {
                        Ok(req.into_response(
                            HttpResponse::Forbidden().finish()
                        ))
                    })
                }
                req.headers_mut().insert(HeaderName::from_static("x-user"), HeaderValue::from_str(&login).unwrap());
                srv.call(req)
            })
            .wrap(CookieSession::signed(config.cookie_key.as_bytes()).secure(false))
            .service(auth::scope(&config))
            .service(
                Scope::new("/api/v1").service(sse).service(tasks)
                    .service(task_run).service(task_stream).service(task_run_stream).service(task_stop)
            )
            .service(Files::new("/", "public").index_file("index.html"))
    );

    tokio::spawn(async move {
        let mut sighup = signal(SignalKind::hangup()).unwrap();
        loop {
            sighup.recv().await;
            app_state::reload_config(&signal_data);
        }
    });

    server = if let Some(l) = listenfd.take_tcp_listener(0).unwrap() {
        server.listen(l)?
    } else {
        server.bind("127.0.0.1:3000")?
    };

    server.run().await
}

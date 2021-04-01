Taru—a dead simple task runner
==============================

Do you suffer from people pestering you to run commands they don't have access to but you do? Do you wish that you could just give them a damn button so they don't have to talk to you?
Well, now you can.

Taru is a simple web service that exposes preconfigured commands to a set of users. You can use it from your browser, you can use it as an api, whatever suits you best.
Every user has their own set of permissions, so that you can control every aspect of running the tasks.

It has been designed to let non-admins run some routine administrative tasks. Some of those take a lot of time, so Taru is able to stream their output in real time.

Features and other traits
==========================

  * Only one instance of a task can be run at a time—by design.
  * Task output is streamed raw and unbuffered.
  * Users have separate permissions for each task, for running, viewing the output, and viewing the task's status.
  * You can, but don't have to use the authorization mechanism.
  * Authentication isn't done by Taru, so you can integrate it with anything.
  * All clients can get status updates in real time.
  * Many clients can stream output of one task.

Installation
=============
Taru is written in Rust, so first, build it:

    cargo build --release

Taru exposes its API under `/api`—if you don't want the web interface, all you have to do is run the binary and pass the path to the configuration file as a first argument:

    taru /etc/taru.yml

Taru is first and foremost an API. This repo contains a simple web interface you can use with it. To make things easier, if you have a `public` directory in the working directory of Taru, it will serve it under `/`. This is a completely static website, however, so you can serve it with another http server just as well. It makes an assumption however, that the Taru's api is available at `/api/v1`.

Requirements
============
Taru runs tasks using systemd-run, for two reasons. First, because it allows Taru to properly stop a task. Second, because it prevents starting two instances of the same task, even if the built-in check fails. This means that the system running taru must run a user systemd instance, and that it must have the ability to manage its cgroups (which is not always true in a container).

Configuration
=============
Create a file named `taru.yml` in the working directory of the application (i.e. the directory you're running it from). Example contents:

    heartbeat: 1
    tasks:
      ping:
        command: [ping, -c10, $host]
        meta:
          description: Ping a host
        arguments:
        - name: host
          datatype: Enum
          enum_source: hosts
      hosts:
        command: [cat, some_host_list]
      download_database:
        command: [pg_dump, some, args]
        buffered: false
        headers:
          content-disposition: "attachment; filename=dump.sql"
        meta:
          description: Download a database
          download: true
    users:
      root:
        can_run: [ping]
        can_view_status: [ping, download_database]
        can_view_output: [ping]

Heartbeat
---------
This is an optional setting that tells taru how often to send `Ping` events to subscribers of `/events`. If omitted, Taru will not send these at all.

The value is the interval between pings in seconds.

Configure this if your users experience event stream disconnections. Also note that this doesn't affect output streams.

Tasks
-----
Each task has a name (the key, e.g. `download_database` in the example above), and the following fields:

  * **command** – a list of arguments of the command to execute, the first one being the path to the binary
  * **buffered** – whether to store the output in memory. default: true
  * **headers** – HTTP headers to send with the output
  * **meta** – arbitrary key-value pairs, the bundled web interface uses `desription` for human-readable task descriptions, and `download` to decide whether to download the output immediately when starting the task.

Arguments
---------
Tasks can be parametrizd using a list of arguments. Each argument has to specify its `name` and `datatype`. Available datatypes are `Int`, `String`, and `Enum`.
Please note that using Strings, while possible, can lead to undesirable consequences. Be wary of allowing arbitrary data in parameters. Ints have to be valid 32-bit signed numbers.
Enums take the output from another task, split it by lines, and permit only values being identical to one of the lines. `enum_source` specified the task whose output will be read.

Taru does not run `enum_sources` automatically. You have to first run it at least once, so that an output is available, in order to run a task that requires it. Taru will, however, tell you that the data is not ready if you don't do so.

In order to use an arguments value, pass `$ARG` as a parameter in cmdline, where ARG is the argument's name. Note that undefined values of ARG will simply be ignored and passed to cmdline without substitution.

In other words, if you have one argument, and it's called `host`, with a value of `example`, and a cmdline `[echo, $host, $asdf]`, the cmdline that will be called is `echo example $asdf`.

All endpoints that run tasks accept arguments as either parameters in the url query, or in a request body (in the same format), e.g. `POST /api/v1/task/ping?host=example.org`.

Users
-----
If you define it, Taru will require users to authenticate by setting the X-User header to their username. Typically, this is done by using a reverse proxy, such as nginx, to authenticate using the desired method, then pass the result as X-User.

Each use can has three kinds of permissions:

  * **can_run** – allows the user to start and stop a task
  * **can_view_status** – allows the user to see the task, and its status, on the task list
  * **can_view_output** – allows the user to see the task's output

All three must be specified for each user. Their values are lists of task names the user has the permission for.

API
===
The API is rather simple.

GET /api/v1/tasks
-----------
Returns a dictionary of tasks you have the `can_view_status` permission for. Each task has the following fields:

  * **name** – The task's name (it's the same thing as the dict key).
  * **meta** – Whatever was put into the meta field of the task's configuration.
  * **state** – "new", "running", or "finished"
  * **exit_code** — If the state is "finished" *and* the task wasn't killed by a signal, its exit code. Otherwise null.
  * **can_run** – Whether you're allowed to run the task.
  * **can_view_output** – Whether you're allowed to view the task's output.

POST /api/v1/task/TASK
----------------------
Starts a task named TASK. Requires `can_run` permission.

Returns `200 Ok` if the task has been started successfully.
Returns `405 Conflict` if the task is already running.
Returns `404 Not found` if the task doesn't exist or you're not allowed to run it.

In all cases, the response body is a plain text message.

GET /api/v1/task/TASK/output
----------------------------
Returns (streams) the task's output. If it finishes, the task is not running anymore. Requires `can_view_output` permission.

Returns `404 Not found` if the task doesn't exist or you're not allowed to view its output.

POST /api/v1/task/TASK/output
-----------------------------
Starts a task and returns (streams) its output. Requires both `can_run` and `can_view_output` permissions.

This endpoint combines the above two in one call to avoid race conditions with unbuffered tasks.
If you run an unbuffered task, then attempt to get its output in a separate requests, you won't get the data outputted before the GET request, so just use this one.

GET /api/v1/task/TASK/status
-----------------------------
Waits for a task to complete, then returns the exit code in the response body.

For your convenience, if you add `?check=true` to the url and the exit code is not zero, it will return http status code 520.

POST /api/v1/task/TASK/status
-----------------------------
Starts a task, waits for it to complete, then returns the exit code in the response body.

For your convenience, if you add `?check=true` to the url and the exit code is not zero, it will return http status code 520.

POST /api/v1/task/TASK/stop
---------------------------
Stops a task called TASK. Requires `can_run` permission.

GET /events
-----------
A [server-sent events][sse] endpoint. Yields events in a `["task_name", EVENT]` form. Currently possible events:

  * `"Started"` – The task was just started.
  * `{"ExitStatus": 5}` – The task has finished (with a status code, unless killed in which case it will be `null`).
  * `"UpdateConfig"` – Taru has reloaded its configuration, refresh your task list.

  [sse]: https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events

Other things
============

Taru is supposed to be started from a systemd socket. If run without it, it binds to `0.0.0.0:3000`. If you want to bind to something else, but not use systemd, use [systemfd][].

Taru supports reloading configuration. In order to reconfigure it, send a SIGHUP to its process.

  [systemfd]: https://github.com/mitsuhiko/systemfd

Roadmap
=======

There are some features planned for the future:

  * More responsive web interface
  * Passing the current username to tasks

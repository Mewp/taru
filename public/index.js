async function fetchTasks() {
  const tasks = await (await fetch("/api/v1/tasks")).json()
  let table = document.getElementById('tasks');
  for(let task of tasks) {
    let row = table.insertRow();
    let nameCell = row.insertCell();
    nameCell.textContent = task;
  }
}

const TaskList = Vue.extend({
  template: '#task-list-template',
  data() {
    return {
      tasks: {},
      task_categories: {},
      task_ids: {}
    }
  },

  methods: {
    initSse() {
      this.sseOpened = false;
      this.eventSource = new EventSource('/api/v1/events', { withCredentials: true });

      this.eventSource.addEventListener('ping', (e) => {
        this.sseOpened = true
      })

      this.eventSource.addEventListener('started', (e) => {
        let data = JSON.parse(e.data)
        this.tasks[data.task].state = 'running'
        this.tasks[data.task].argument_values = data.arguments
      })

      this.eventSource.addEventListener('finished', async (e) => {
        let data = JSON.parse(e.data)
        let task = this.tasks[data.task]
        task.state = 'finished'
        task.exit_code = data.exit_code
        if(!this.$root.$data.task_outputs.hasOwnProperty(data.task)) return;
        let resp = await fetch(`/api/v1/task/${data.task}/output`);
        let text = await resp.text();
        this.$set(this.$root.$data.task_outputs, data.task, text.trim().split("\n"));
        for(let task of this.$refs.tasks) {
          for(let arg of task.task.arguments) {
            if(arg.enum_source == data.task) {
              task.updateArgs();
            }
          }
        }
      })

      this.eventSource.addEventListener('update_config', async () => {
        let tasks = await (await fetch("/api/v1/tasks")).json()
        let categories = {};
        for(let task of Object.keys(tasks).sort()) {
          this.$set(this.tasks, task, tasks[task])
          let category = tasks[task].meta?.category || '';
          categories[category] = categories[category] || [];
          categories[category].push(task);
        }

        for(let task in this.tasks) {
          if(!tasks[task]) {
            this.$delete(this.tasks, task)
          }
        }
        this.task_categories = categories;
      })

      this.eventSource.addEventListener('change_data', () => {
        let data = JSON.parse(e.data)
        let task = this.tasks[data[0]]
        task.data[data[1]] = data[2]
      })

      this.eventSource.onerror = (e) => {
        if(this.sseOpened) {
          this.initSse()
        } else {
          document.location.reload()
        }
      }
    }
  },

  async mounted() {
    let tasks = await (await fetch("/api/v1/tasks")).json()
    this.tasks = this.$root.$data.tasks;
    let categories = {};
    for(let task of Object.keys(tasks).sort()) {
      this.$set(this.tasks, task, tasks[task])
      let category = tasks[task].meta?.category || '';
      categories[category] = categories[category] || [];
      categories[category].push(task);
    }

    this.task_categories = categories;

    this.initSse()

    // Because EventSource doesn't have an onclose method, we have to poll it.
    setInterval(() => {
      if(this.eventSource.readyState == EventSource.CLOSED) {
        if(this.sseOpened) {
          this.initSse()
        } else {
          document.location.reload()
        }
      }
    }, 1000)

    window.addEventListener('beforeunload', () => {
      this.eventSource.close()
    })
  }
})

Vue.component('task', {
  template: '#task-template',
  props: ['name', 'task'],
  data() {
    return {
      args: {},
      arg_values: {},
      output_shown: false,
      since: null,
      interval: null,
    }
  },

  async mounted() {
    if(window.location.hash == `#task/${this.name}/output`) {
      this.output_shown = true
    }
    for(let arg of this.task.arguments) {
      if(this.$root.$data.task_outputs[arg.enum_source] === undefined) {
        const promise = new Promise(async (resolve, reject) => {
          resp = await fetch(`/api/v1/task/${arg.enum_source}/output`, {method: 'POST'});
          if(!resp.ok) return reject();
          data = await resp.text();
          data = data.trim().split("\n");
          this.$set(this.$root.$data.task_outputs, arg.enum_source, data);
          resolve(data);
        });
        this.$root.$data.task_outputs[arg.enum_source] = promise;
      }
      const data = await this.$root.$data.task_outputs[arg.enum_source];
      this.$set(this.args, arg.name, data[0]);
      this.arg_values[arg.enum_source] = data;
    }
  },

  methods: {
    updateArgs() {
      for(let arg of this.task.arguments) {
        const data = this.$root.$data.task_outputs[arg.enum_source];
        const val = this.args[arg.name];
        // The select doesn't seem to update properly when this value isn't actually changed.
        // So we make sure it changes.
        this.$set(this.args, arg.name, null);
        if(data.includes(val)) {
          this.$set(this.args, arg.name, val);
        } else {
          this.$set(this.args, arg.name, data[0]);
        }
        this.arg_values[arg.enum_source] = data;
      }
    },

    run() {
      let params = new URLSearchParams("");
      for(let arg in this.args) {
        params.append(arg, this.args[arg]);
      }
      fetch(`/api/v1/task/${this.name}?${params}`, {method: 'POST'})
    },

    stop() {
      fetch(`/api/v1/task/${this.name}/stop`, {method: 'POST'})
    },

    show_output() {
      window.location = `#task/${this.name}/output`
    },
  }
})

const TaskOutput = Vue.extend({
  template: `
    <div class="output" ref="output"></div>
  `,
  props: ['name'],
  data() {
    return {
      output: ""
    }
  },

  async mounted() {
    let resp = await fetch(`/api/v1/task/${this.name}/output`)
    let reader = resp.body.getReader()
    var {done, value} = await reader.read()

    const term = new Terminal({convertEol: true, theme: {background: '#1b1d1e'}});
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(this.$refs.output);
    fitAddon.fit();

    while(!done) {
      term.write(value);
      var {done, value} = await reader.read()
    }
  }
})

const routes = [
  { path: '/', component: TaskList },
  { path: '/task/:name/output', component: TaskOutput, props: true }
]

const router = new VueRouter({ routes })

Vue.component('vue-select', window.VueSelect.VueSelect)
var app = new Vue({
  el: '#app',
  data: {tasks: {}, task_outputs: {}},
  router
})

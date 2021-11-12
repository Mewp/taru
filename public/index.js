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

      this.eventSource.addEventListener('finished', (e) => {
        let data = JSON.parse(e.data)
        let task = this.tasks[data.task]
        task.state = 'finished'
        task.exit_code = data.exit_code
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
      console.log(tasks[task].data)
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
      output_shown: false,
      since: null,
      interval: null,
    }
  },

  async mounted() {
    if(window.location.hash == `#task/${this.name}/output`) {
      this.output_shown = true
    }
  },

  asyncComputed: {
    arg_values: {
      default: [],
      lazy: true,
      async get() {
        for(let arg of this.task.arguments) {
          if(this.$root.$data.task_outputs[arg.enum_source] === undefined) {
            const promise = fetch(`/api/v1/task/${arg.enum_source}/output`, {method: 'POST'}).then((resp) => {
              if(!resp.ok) return;
              return resp.text();
            }).then((data) => {
              data = data.trim().split("\n");
              this.$set(this.$root.$data.task_outputs, arg.enum_source, data);
            });
            this.$root.$data.task_outputs[arg.enum_source] = promise;
          }
          const data = await this.$root.$data.task_outputs[arg.enum_source];
          this.$set(this.args, arg.name, this.$root.$data.task_outputs[arg.enum_source][0]);
        }

        return this.$root.$data.task_outputs
      }
    }
  },

  methods: {
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

    const term = new Terminal({theme: {background: '#1b1d1e'}});
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
Vue.use(AsyncComputed)
var app = new Vue({
  el: '#app',
  data: {tasks: {}, task_outputs: {}},
  router
})

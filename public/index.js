async function fetchTasks() {
  const tasks = await (await fetch("/api/v1/tasks")).json()
  let table = document.getElementById('tasks'); 
  for(let task of tasks) {
    let row = table.insertRow();
    let nameCell = row.insertCell();
    nameCell.textContent = task;
  }
}

function sortObject(obj) {
    return Object.keys(obj).sort().reduce(function (result, key) {
        result[key] = obj[key];
        return result;
    }, {});
}

const TaskList = Vue.extend({
  template: `<table class="task-list">
    <task v-for="(task, name) in tasks" ref="tasks" :task="task" :name="name" :key="name" />
  </table>`,
  data() {
    return {
      tasks: {},
      task_ids: {}
    }
  },

  methods: {
    initSse() {
      this.sseOpened = false;
      this.eventSource = new EventSource('/api/v1/events', { withCredentials: true });
      this.eventSource.onmessage = (e) => {
        let data = JSON.parse(e.data)
        let task = this.tasks[data[0]]
        console.log(data, task)
        if(data[1] == 'Started') {
          task.state = 'running'
        } else {
          task.exit_code = data[1].Finished
          task.state = 'finished'
        }
      }
      this.eventSource.onopen = () => {
        this.sseOpened = true
      }
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
    for(task in sortObject(tasks)) {
      this.$set(this.tasks, task, tasks[task])
    }

    this.initSse()
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
  template: `
    <tr>
      <td>
        <span>{{task.meta?.description || name}}</span>
      </td>
      <td class="actions">
        <div class="btn btn-primary" v-if="task.can_run">
          <form v-if="task.meta?.download" method="POST" v-bind:action="'/api/v1/task/' + task.name + '/output'">
            <button v-unless="task.state == 'running'"><i class="material-icons">cloud_download</i></button>
          </form>
          <i v-else-if="task.state == 'running'" v-on:click="stop" class="material-icons">stop</i>
          <i v-else v-on:click="run" class="material-icons">play_arrow</i>
        </div>
        <router-link
          :to="'/task/' + task.name + '/output'"
          class="btn"
          v-if="task.can_view_output && !task.meta?.download"
          title="Show output"><i class="material-icons">search</i>
        </router-link>
        <a :href="'/api/v1/task/' + task.name + '/output'"
           class="btn"
           v-if="task.can_view_output && !task.meta?.download" title="Get raw output">
            <i class="material-icons">code</i>
        </a>
      </td>
      <td>
        <span v-if="task.state == 'running'"><img src="/loading.svg"></span>
        <span v-if="task.state == 'finished' && task.exit_code !== null">Finished with exit code {{task.exit_code}}</span>
        <span v-if="task.state == 'finished' && task.exit_code === null">Stopped</span>
      </td>
    </tr>
  `,
  props: ['name', 'task'],
  data() {
    return {
      output_shown: false
    }
  },

  mounted() {
    if(window.location.hash == `#task/${this.name}/output`) {
      this.output_shown = true
    }
  },

  methods: {
    run() {
      fetch(`/api/v1/task/${this.name}`, {method: 'POST'})
    },

    stop() {
      fetch(`/api/v1/task/${this.name}/stop`, {method: 'POST'})
    },

    show_output() {
      window.location = `#task/${this.name}/output`
    }
  }
})

const TaskOutput = Vue.extend({
  template: `
    <div class="output" v-html="output"></div>
  `,
  props: ['name'],
  data() {
    return {
      output: ""
    }
  },

  async mounted() {
    console.log(this.name)
    let resp = await fetch(`/api/v1/task/${this.name}/output`)
    let reader = resp.body.getReader()
    var {done, value} = await reader.read()
    let decoder = new TextDecoder();
    let current_color = 7;
    let bold = false;
    let colors = [
      ['black', '#cd0000', '#00cd00', '#cdcd00', '#6D6DFF', '#cd00cd', '#00cdcd', '#D3D7CF'],
      ['#7f7f7f', '#ff0000', '#00ff00', '#FCE94F', '#5C5CFF', '#ff00ff', '#00ffff', '#ffffff']
    ]
    let output = "<span>";
    while(!done) {
      let start = 0;
      let i = 0;
      for(; i < value.length; i++) {
        if(value[i] != 0x1b) continue
        output += decoder.decode(value.slice(start, i));
        i++
        start = i
        if(value[i] != 0x5b) continue // '['
        // If the command starts with a number, parse it.
        let num = 0
        while(value[++i] >= 0x30 && value[i] <= 0x39) {
          num *= 10
          num += value[i] - 0x30
        }
        if(value[i] == 0x6d) { // Select Graphic Rendition, what we're interested in.
          if(num == 0) {
            bold = false
            current_color = 7
            output += '</span><span>'
          } else if(num == 1) {
            bold = true
            output += `</span><span style="color: ${colors[+bold][current_color]}; font-weight: 700">`
          } else if(num >= 30 && num <= 37) {
            output += `<span style="color: ${colors[+bold][num-30]}">`
          }
        }
        start = i+1
      }
      output += decoder.decode(value.slice(start, i))
      this.output = output + "</span>";
      var {done, value} = await reader.read()
    }
  }
})

const routes = [
  { path: '/', component: TaskList },
  { path: '/task/:name/output', component: TaskOutput, props: true }
]

const router = new VueRouter({ routes })

var app = new Vue({el: '#app', router})

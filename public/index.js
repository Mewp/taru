async function fetchTasks() {
  const tasks = await (await fetch("/api/v1/tasks")).json()
  let table = document.getElementById('tasks'); 
  for(let task of tasks) {
    let row = table.insertRow();
    let nameCell = row.insertCell();
    nameCell.textContent = task;
  }
}

Vue.component('task-list', {
  template: `<table class="task-list">
    <task v-for="(task, name) in tasks" ref="tasks" :task="task" :name="name" :key="name" />
  </table>`,
  data() {
    return {
      tasks: {},
      task_ids: {}
    }
  },

  async mounted() {
    let tasks = await (await fetch("/api/v1/tasks")).json()
    for(task in tasks) {
      this.$set(this.tasks, task, tasks[task])
    }

    let eventSource = new EventSource('/api/v1/events', { withCredentials: true });
    eventSource.onmessage = (e) => {
      let data = JSON.parse(e.data)
      let task = this.tasks[data[0]]
      console.log(data, task)
      if(data[1] == 'Started') {
        task.state = 'running'
      } else {
        task.exit_code = data[1].ExitStatus
        task.state = 'finished'
      }
    }
    window.addEventListener('beforeunload', function() {
      eventSource.close()
    })
  }
})

Vue.component('task', {
  template: `
    <tr>
      <td>
        <a v-if="task.can_view_output && !task.meta?.download" :href="'/api/v1/task/' + task.name + '/output'">{{task.meta?.description || name}}</a>
        <span v-else>{{task.meta?.description || name}}</span>
      </td>
      <td>
        <div class="task-run-button" v-if="task.can_run">
          <form v-if="task.meta?.download" method="POST" v-bind:action="'/api/v1/task/' + task.name + '/output'">
            <button v-unless="task.state == 'running'"><i class="material-icons">cloud_download</i></button>
          </form>
          <i v-else-if="task.state == 'running'" v-on:click="stop" class="material-icons">stop</i>
          <i v-else v-on:click="run" class="material-icons">play_arrow</i>
        </div>
      <td>
        <span v-if="task.state == 'running'">Running</span>
        <span v-if="task.state == 'finished' && task.exit_code !== null">Finished with exit code {{task.exit_code}}</span>
        <span v-if="task.state == 'finished' && task.exit_code === null">Stopped</span>
      </td>
    </tr>
  `,
  props: ['name', 'task'],

  methods: {
    run() {
      fetch(`/api/v1/task/${this.name}`, {method: 'POST'})
    },

    stop() {
      fetch(`/api/v1/task/${this.name}/stop`, {method: 'POST'})
    }
  }
})

var app = new Vue({el: '#app'})

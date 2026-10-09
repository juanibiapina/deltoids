interface Task {
  run: () => void;
}

const tasks: Task[] = [];
let scheduled = false;

function request(callback: () => void): void {
  if (typeof requestIdleCallback === "function") requestIdleCallback(callback);
  else setTimeout(callback, 0);
}

function pump(): void {
  scheduled = false;
  tasks.shift()?.run();
  if (tasks.length > 0) schedule();
}

function schedule(): void {
  if (scheduled) return;
  scheduled = true;
  request(pump);
}

export function whenIdle(run: () => void): () => void {
  const task = { run };
  tasks.push(task);
  schedule();
  return () => {
    const at = tasks.indexOf(task);
    if (at >= 0) tasks.splice(at, 1);
  };
}

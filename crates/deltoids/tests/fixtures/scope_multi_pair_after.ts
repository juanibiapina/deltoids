import { NEW } from './constants';

export class TaskService {
  private processTask(task: Task) {
    const mapping = {
      [TaskType.TYPE_A]: NEW.a.value,
      [TaskType.TYPE_B]: NEW.b.value,
      [TaskType.TYPE_C]: NEW.c.value,
    };

    return mapping[task.type];
  }
}

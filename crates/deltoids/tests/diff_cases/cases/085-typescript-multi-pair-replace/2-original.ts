import { OLD_A, OLD_B, OLD_C } from './constants';

export class TaskService {
  private processTask(task: Task) {
    const mapping = {
      [TaskType.TYPE_A]: OLD_A,
      [TaskType.TYPE_B]: OLD_B,
      [TaskType.TYPE_C]:
        OLD_C,
    };

    return mapping[task.type];
  }
}

// Returns a typed UserDO stub from env with automatic retry on transient
// DO errors. Centralised so callers don't repeat the idFromName + get dance.

import type { UserDO } from "./index";
import type { Env } from "../types";
import { withDORetry } from "../do/retry";

export type UserDOStub = DurableObjectStub<UserDO>;

export const getUserDO = (env: Env, clerkUserId: string): UserDOStub =>
  withDORetry(() => {
    const id = env.USER_DO.idFromName(clerkUserId);
    return env.USER_DO.get(id);
  });

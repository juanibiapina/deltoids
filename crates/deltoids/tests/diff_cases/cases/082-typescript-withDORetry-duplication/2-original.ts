// Returns a typed UserDO stub from env. Centralised so callers don't
// repeat the idFromName + get dance.

import type { UserDO } from "./index";
import type { Env } from "../types";

export type UserDOStub = DurableObjectStub<UserDO>;

export const getUserDO = (env: Env, clerkUserId: string): UserDOStub => {
  const id = env.USER_DO.idFromName(clerkUserId);
  return env.USER_DO.get(id);
};

import { createProxy } from "./proxy";

const proxy = createProxy(
  ["ANTHROPIC_API_KEY", "BRAVE_API_KEY"],
  ["GOOGLE_WORKSPACE_CLI_TOKEN", "GH_TOKEN"],
);

export function useProxy() {
  return proxy;
}

import * as React from "react";
import { Screenshot } from "./Screenshot";

/**
 * Same `edit-tui` screenshot as the hero, used here to anchor the agent
 * traces feature. Reusing the image keeps the page coherent: one TUI,
 * two stories about it (hero: "look at it", agents: "this is how you
 * review your agent's edits").
 */
export function AgentTraceMock() {
  return (
    <Screenshot
      src="/screenshots/edit-tui.png"
      alt="edit-tui browsing an agent trace: left pane lists each edit with a one-line reason; right pane shows the diff with deltoids' boxed scope context."
      width={4000}
      height={2464}
    />
  );
}

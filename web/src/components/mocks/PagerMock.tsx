import * as React from "react";
import { Screenshot } from "./Screenshot";

/**
 * Real screenshot of lazygit running with `deltoids` configured as the
 * pager: branches/files/commits panes on the left, the deltoids-rendered
 * diff with its boxed scope context on the right.
 */
export function PagerMock() {
  return (
    <Screenshot
      src="/screenshots/lazygit.png"
      alt="Lazygit with deltoids configured as the pager. The right pane shows a unified diff rendered by deltoids, complete with the cyan boxed scope context that names the enclosing block."
      width={4000}
      height={2678}
    />
  );
}

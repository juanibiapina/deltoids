import * as React from "react";

/**
 * GitHub mark. lucide-react dropped brand icons, so we ship a tiny inline
 * one. Sized via the standard size-* utilities to match lucide-react icons
 * (16px when used inside Button via `[&_svg]:size-4`).
 */
export function GitHubIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden
      {...props}
    >
      <path d="M12 .5C5.73.5.75 5.48.75 11.75c0 4.96 3.21 9.16 7.66 10.65.56.1.77-.24.77-.54v-1.9c-3.12.68-3.78-1.5-3.78-1.5-.51-1.3-1.25-1.65-1.25-1.65-1.02-.7.08-.69.08-.69 1.13.08 1.72 1.16 1.72 1.16 1 1.71 2.63 1.22 3.27.93.1-.73.39-1.22.71-1.5-2.49-.28-5.11-1.25-5.11-5.55 0-1.23.44-2.23 1.16-3.02-.12-.29-.5-1.43.11-2.99 0 0 .94-.3 3.08 1.15a10.7 10.7 0 0 1 5.6 0c2.13-1.45 3.07-1.15 3.07-1.15.61 1.56.23 2.7.11 2.99.72.79 1.16 1.79 1.16 3.02 0 4.31-2.63 5.27-5.13 5.54.4.34.76 1.02.76 2.06v3.05c0 .3.21.65.78.54A11.26 11.26 0 0 0 23.25 11.75C23.25 5.48 18.27.5 12 .5z" />
    </svg>
  );
}

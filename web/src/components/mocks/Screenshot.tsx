import * as React from "react";
import Image, { type StaticImageData } from "next/image";
import { cn } from "@/lib/utils";

/**
 * Renders a real product screenshot with the standard marketing-page
 * framing: rounded card, hairline ring, drop shadow, and a soft accent
 * glow tucked underneath. The screenshots already include their own
 * window chrome (macOS dots / tmux border), so we deliberately do NOT
 * wrap them in another window.
 */
export function Screenshot({
  src,
  alt,
  width,
  height,
  priority = false,
  className,
}: {
  src: string | StaticImageData;
  alt: string;
  /** Intrinsic width of the image file. Used for next/image layout. */
  width: number;
  /** Intrinsic height of the image file. */
  height: number;
  /** Use on the hero so it's not lazy-loaded. */
  priority?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("relative", className)}>
      {/* Soft accent glow tucked under the image. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-x-8 -bottom-8 h-24 rounded-[40%] bg-accent/30 blur-3xl"
      />
      <div className="relative overflow-hidden rounded-xl border border-line bg-bg-elev shadow-2xl shadow-black/40 ring-1 ring-white/[0.04]">
        <Image
          src={src}
          alt={alt}
          width={width}
          height={height}
          priority={priority}
          sizes="(max-width: 72rem) 100vw, 72rem"
          className="h-auto w-full"
        />
      </div>
    </div>
  );
}

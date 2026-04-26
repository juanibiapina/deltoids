"use client";

import * as React from "react";
import { motion, type HTMLMotionProps } from "motion/react";

type RevealProps = HTMLMotionProps<"div"> & {
  delay?: number;
  /**
   * Distance to slide up from, in pixels. Set to 0 to fade only.
   */
  y?: number;
};

/**
 * Fade + slide-in on viewport entry. Plays once. Used to reveal section
 * blocks and product mocks as the user scrolls.
 */
export function Reveal({
  delay = 0,
  y = 16,
  children,
  ...rest
}: RevealProps) {
  return (
    <motion.div
      initial={{ opacity: 0, y }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, margin: "-10% 0px" }}
      transition={{
        duration: 0.6,
        ease: [0.16, 1, 0.3, 1],
        delay,
      }}
      {...rest}
    >
      {children}
    </motion.div>
  );
}

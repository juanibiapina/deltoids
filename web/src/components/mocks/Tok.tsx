import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * Hand-tokenized syntax span. The brief explicitly forbids Shiki / Prism
 * (and screenshots), so every "highlighted" character is wrapped in <Tok>
 * with a kind that resolves to a Tailwind color utility.
 */
export type TokKind =
  | "keyword"
  | "fn"
  | "type"
  | "string"
  | "number"
  | "comment"
  | "punct"
  | "prop"
  | "ident"
  | "plain"
  | "dim";

const KIND_CLASS: Record<TokKind, string> = {
  keyword: "text-tok-keyword",
  fn: "text-tok-fn",
  type: "text-tok-type",
  string: "text-tok-string",
  number: "text-tok-number",
  comment: "text-tok-comment italic",
  punct: "text-tok-punct",
  prop: "text-tok-prop",
  ident: "text-tok-ident",
  plain: "text-fg",
  dim: "text-fg-dim",
};

export function Tok({
  kind = "plain",
  children,
  className,
}: {
  kind?: TokKind;
  children: React.ReactNode;
  className?: string;
}) {
  return <span className={cn(KIND_CLASS[kind], className)}>{children}</span>;
}

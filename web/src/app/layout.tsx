import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import { Analytics } from "@vercel/analytics/next";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

const SITE_URL = "https://deltoids.dev";

export const metadata: Metadata = {
  metadataBase: new URL(SITE_URL),
  title: {
    default: "deltoids — diffs with context",
    template: "%s — deltoids",
  },
  description:
    "deltoids expands every hunk in a unified diff to include the entire enclosing function, class, or block. Tree-sitter resolved. Pipe git diff, gh pr diff, or set as your pager.",
  applicationName: "deltoids",
  authors: [{ name: "Juan Ibiapina", url: "https://github.com/juanibiapina" }],
  keywords: [
    "diff",
    "git",
    "tree-sitter",
    "code review",
    "lazygit",
    "pager",
    "agents",
    "coding agent",
  ],
  openGraph: {
    type: "website",
    url: SITE_URL,
    siteName: "deltoids",
    title: "deltoids — diffs with context",
    description:
      "Tree-sitter-aware diff pager. See the whole enclosing function, not just three lines of context.",
  },
  twitter: {
    card: "summary_large_image",
    title: "deltoids — diffs with context",
    description:
      "Tree-sitter-aware diff pager. See the whole enclosing function, not just three lines of context.",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`${geistSans.variable} ${geistMono.variable} antialiased`}
    >
      <body className="min-h-screen bg-bg text-fg">
        {children}
        <Analytics />
      </body>
    </html>
  );
}

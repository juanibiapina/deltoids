import { ImageResponse } from "next/og";
import type { NextRequest } from "next/server";

export const runtime = "edge";

/**
 * Dynamic OG endpoint. Accepts ?title= and ?subtitle= so future pages can
 * generate their own social cards without duplicating layout. Same visual
 * language as the static landing card (opengraph-image.tsx).
 */
export async function GET(req: NextRequest) {
  const { searchParams } = new URL(req.url);
  const title = (searchParams.get("title") ?? "deltoids").slice(0, 80);
  const subtitle = (
    searchParams.get("subtitle") ?? "Diffs with context."
  ).slice(0, 160);

  return new ImageResponse(
    (
      <div
        style={{
          height: "100%",
          width: "100%",
          display: "flex",
          flexDirection: "column",
          padding: "72px",
          background:
            "linear-gradient(135deg, #1a1b26 0%, #1f2335 60%, #232639 100%)",
          color: "#c0caf5",
          fontFamily: "ui-sans-serif, system-ui, sans-serif",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: "16px" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "40px",
              height: "40px",
              borderRadius: "8px",
              background: "linear-gradient(135deg, #7aa2f7 0%, #b4d0ff 100%)",
              color: "#1a1b26",
              fontWeight: 700,
              fontSize: "22px",
            }}
          >
            Δ
          </div>
          <div
            style={{
              fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
              fontSize: "26px",
              fontWeight: 600,
            }}
          >
            deltoids
          </div>
        </div>

        <div
          style={{
            display: "flex",
            flexDirection: "column",
            marginTop: "auto",
          }}
        >
          <div
            style={{
              fontSize: "84px",
              fontWeight: 600,
              lineHeight: 1.05,
              letterSpacing: "-0.025em",
              maxWidth: "1056px",
            }}
          >
            {title}
          </div>
          <div
            style={{
              marginTop: "28px",
              fontSize: "30px",
              color: "#9aa5ce",
              maxWidth: "1056px",
              lineHeight: 1.4,
            }}
          >
            {subtitle}
          </div>
        </div>
      </div>
    ),
    {
      width: 1200,
      height: 630,
    },
  );
}

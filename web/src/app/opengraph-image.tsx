import { ImageResponse } from "next/og";

export const runtime = "edge";
export const alt = "deltoids — diffs with context";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

/**
 * Static OG card for the landing route. Hand-rendered (no screenshots):
 * a tiny mini-diff sits inside a card on a tokyonight gradient.
 */
export default async function OG() {
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
        {/* Brand row */}
        <div style={{ display: "flex", alignItems: "center", gap: "16px" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "40px",
              height: "40px",
              borderRadius: "8px",
              background:
                "linear-gradient(135deg, #7aa2f7 0%, #b4d0ff 100%)",
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
              letterSpacing: "-0.01em",
            }}
          >
            deltoids
          </div>
        </div>

        {/* Headline */}
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            marginTop: "auto",
          }}
        >
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              alignItems: "baseline",
              fontSize: "84px",
              fontWeight: 600,
              lineHeight: 1.05,
              letterSpacing: "-0.025em",
              maxWidth: "880px",
            }}
          >
            <span style={{ marginRight: "0.35em" }}>Diffs with</span>
            <span
              style={{
                background:
                  "linear-gradient(90deg, #b4d0ff 0%, #7aa2f7 100%)",
                backgroundClip: "text",
                color: "transparent",
              }}
            >
              context.
            </span>
          </div>
          <div
            style={{
              marginTop: "28px",
              fontSize: "30px",
              color: "#9aa5ce",
              maxWidth: "880px",
              lineHeight: 1.4,
            }}
          >
            Tree-sitter-aware diff pager. See the whole enclosing function,
            not three lines of context.
          </div>
        </div>

        {/* Mini-diff card */}
        <div
          style={{
            position: "absolute",
            right: "72px",
            top: "120px",
            display: "flex",
            flexDirection: "column",
            width: "440px",
            borderRadius: "14px",
            border: "1px solid #2e3148",
            background: "#1f2335",
            overflow: "hidden",
            fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
            fontSize: "18px",
            boxShadow: "0 30px 60px rgba(0,0,0,0.45)",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: "8px",
              padding: "10px 14px",
              background: "#232639",
              borderBottom: "1px solid #2e3148",
            }}
          >
            <div
              style={{
                width: "10px",
                height: "10px",
                borderRadius: "5px",
                background: "#ff5f57",
              }}
            />
            <div
              style={{
                width: "10px",
                height: "10px",
                borderRadius: "5px",
                background: "#febc2e",
              }}
            />
            <div
              style={{
                width: "10px",
                height: "10px",
                borderRadius: "5px",
                background: "#28c840",
              }}
            />
            <div
              style={{
                marginLeft: "10px",
                color: "#565f89",
                fontSize: "14px",
              }}
            >
              scope.rs
            </div>
          </div>
          <div style={{ display: "flex", flexDirection: "column", padding: "14px 16px" }}>
            <div style={{ color: "#7aa2f7", display: "flex" }}>
              {"@@ -147 +147 @@ fn collect_insert_lines"}
            </div>
            <div style={{ color: "#9aa5ce", display: "flex" }}>
              {"  fn collect_insert_lines(range) {"}
            </div>
            <div
              style={{
                background: "rgba(244,63,94,0.12)",
                color: "#fb7185",
                display: "flex",
              }}
            >
              {"-     if range.is_empty() { return; }"}
            </div>
            <div
              style={{
                background: "rgba(16,185,129,0.12)",
                color: "#6ee7b7",
                display: "flex",
              }}
            >
              {"+     let budget = MAX.min(range.len());"}
            </div>
            <div
              style={{
                background: "rgba(16,185,129,0.12)",
                color: "#6ee7b7",
                display: "flex",
              }}
            >
              {"+     if budget == 0 { return; }"}
            </div>
            <div style={{ color: "#9aa5ce", display: "flex" }}>{"  }"}</div>
          </div>
        </div>
      </div>
    ),
    {
      ...size,
    },
  );
}

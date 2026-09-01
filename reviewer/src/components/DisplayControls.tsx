import type { Prefs } from "../hooks/usePrefs";
import { SYNTAX_THEMES } from "../core/themes";

interface DisplayControlsProps {
  prefs: Prefs;
  hasToken: boolean;
  onToken: () => void;
  // "bar": a flat inline row for wide screens. "menu": labeled rows for the
  // popover on narrow screens.
  variant: "bar" | "menu";
  selectId?: string;
}

// The display controls (wrap, line numbers, text size, syntax theme, chrome
// theme, token) shared between the inline toolbar and the settings popover, so
// the two never drift.
export function DisplayControls({
  prefs,
  hasToken,
  onToken,
  variant,
  selectId,
}: DisplayControlsProps) {
  const menu = variant === "menu";
  const wrap = (label: string, control: React.ReactNode) =>
    menu ? (
      <div className="settings-row">
        <span className="settings-label">{label}</span>
        {control}
      </div>
    ) : (
      control
    );

  const sizeGroup = (
    <div className="size-cycle" role="group" aria-label="Text size">
      <button
        type="button"
        className="tool"
        title="Smaller text"
        disabled={prefs.sizeIndex === 0}
        onClick={() => prefs.stepSize(-1)}
      >
        A<span className="minus">−</span>
      </button>
      <button
        type="button"
        className="tool"
        title="Larger text"
        disabled={prefs.sizeIndex === 2}
        onClick={() => prefs.stepSize(1)}
      >
        A<span className="plus">+</span>
      </button>
    </div>
  );

  const themeSelect = (
    <select
      id={selectId}
      className="tool theme-select"
      aria-label="Syntax theme"
      title="Syntax theme"
      value={prefs.syntaxThemeChoice ?? ""}
      onChange={(e) =>
        prefs.setSyntaxTheme(e.target.value === "" ? null : e.target.value)
      }
    >
      <option value="">Auto ({prefs.syntaxTheme})</option>
      <optgroup label="Dark">
        {SYNTAX_THEMES.dark.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </optgroup>
      <optgroup label="Light">
        {SYNTAX_THEMES.light.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </optgroup>
    </select>
  );

  return (
    <>
      <div className={menu ? "settings-row" : "tool-group"}>
        <button
          type="button"
          className="tool"
          aria-pressed={!prefs.nowrap}
          title="Wrap long lines"
          onClick={prefs.toggleWrap}
        >
          Wrap
        </button>
        <button
          type="button"
          className="tool"
          aria-pressed={prefs.hideLineNumbers}
          title="Show line numbers on diff rows"
          onClick={prefs.toggleLineNumbers}
        >
          Line #
        </button>
        {!menu && sizeGroup}
      </div>
      {menu && wrap("Text size", sizeGroup)}
      {wrap("Syntax theme", themeSelect)}
      <div className={menu ? "settings-row" : "tool-group"}>
        <button
          type="button"
          className="tool"
          aria-pressed={prefs.theme === "light"}
          title={
            prefs.theme === "dark"
              ? "Switch to light theme"
              : "Switch to dark theme"
          }
          onClick={prefs.toggleTheme}
        >
          {prefs.theme === "dark" ? "☾" : "☀"}
          {menu ? (prefs.theme === "dark" ? " Dark" : " Light") : ""}
        </button>
        <button
          type="button"
          className={`tool${hasToken ? " has-token" : ""}`}
          title="GitHub token"
          onClick={onToken}
        >
          🔑{menu ? (hasToken ? " Token set" : " Token") : ""}
        </button>
      </div>
    </>
  );
}

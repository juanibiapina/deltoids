import { type FormEvent, type Ref, type RefObject } from "react";
import type { Prefs } from "../hooks/usePrefs";

interface TopbarProps {
  topbarRef: RefObject<HTMLElement | null>;
  input: string;
  onInput: (value: string) => void;
  onSubmit: () => void;
  hasToken: boolean;
  onToken: () => void;
  showToolbar: boolean;
  prefs: Prefs;
  onFilesToggle: () => void;
  drawerOpen: boolean;
}

export function Topbar({
  topbarRef,
  input,
  onInput,
  onSubmit,
  hasToken,
  onToken,
  showToolbar,
  prefs,
  onFilesToggle,
  drawerOpen,
}: TopbarProps) {
  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit();
  };

  return (
    <header className="topbar" ref={topbarRef as Ref<HTMLElement>}>
      <div className="topbar-inner">
        <div className="brand">
          deltoids<span className="brand-dim"> review</span>
        </div>
        <form className="pr-form" onSubmit={handleSubmit}>
          <input
            className="pr-input"
            type="text"
            placeholder="github.com/owner/repo/pull/123"
            autoComplete="off"
            spellCheck={false}
            value={input}
            onChange={(e) => onInput(e.target.value)}
          />
          <button type="submit">Review</button>
          <button
            type="button"
            title="GitHub token"
            className={hasToken ? "has-token" : undefined}
            id="token-btn"
            onClick={onToken}
          >
            <span className="token-glyph">🔑</span>
            <span className="token-dot" aria-hidden="true"></span>
          </button>
        </form>
      </div>
      {showToolbar && (
        <div className="toolbar">
          <button
            className="tool"
            type="button"
            aria-expanded={drawerOpen}
            id="files-btn"
            onClick={onFilesToggle}
          >
            Files
          </button>
          <div className="tool-group">
            <button
              className="tool"
              type="button"
              aria-pressed={!prefs.nowrap}
              title="Wrap long lines"
              onClick={prefs.toggleWrap}
            >
              Wrap
            </button>
            <div className="size-cycle" role="group" aria-label="Text size">
              <button
                className="tool"
                type="button"
                title="Smaller text"
                disabled={prefs.sizeIndex === 0}
                onClick={() => prefs.stepSize(-1)}
              >
                A<span className="minus">−</span>
              </button>
              <button
                className="tool"
                type="button"
                title="Larger text"
                disabled={prefs.sizeIndex === 2}
                onClick={() => prefs.stepSize(1)}
              >
                A<span className="plus">+</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </header>
  );
}

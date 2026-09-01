import { type FormEvent, type Ref, type RefObject, useRef, useState } from "react";
import type { Prefs } from "../hooks/usePrefs";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { SettingsMenu } from "./SettingsMenu";
import { DisplayControls } from "./DisplayControls";

interface TopbarProps {
  topbarRef: RefObject<HTMLElement | null>;
  input: string;
  onInput: (value: string) => void;
  onSubmit: () => void;
  hasToken: boolean;
  onToken: () => void;
  started: boolean;
  prefs: Prefs;
  onFilesToggle: () => void;
  drawerOpen: boolean;
  onSettingsOpenChange?: (open: boolean) => void;
}

export function Topbar({
  topbarRef,
  input,
  onInput,
  onSubmit,
  hasToken,
  onToken,
  started,
  prefs,
  onFilesToggle,
  drawerOpen,
  onSettingsOpenChange,
}: TopbarProps) {
  // Wide screens have room to show every control inline; narrow screens fold
  // them into the settings popover so the bar stays one row.
  const wide = useMediaQuery("(min-width: 640px)", true);

  // On narrow screens the URL field folds away after a PR loads; a search
  // affordance brings it back. Wide screens always keep it visible.
  const [editing, setEditing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const formOpen = wide || !started || editing;

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit();
    setEditing(false);
  };

  const openEditor = () => {
    setEditing(true);
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  return (
    <header className="topbar" ref={topbarRef as Ref<HTMLElement>}>
      <div className="topbar-inner">
        <div className="brand">
          deltoids<span className="brand-dim"> review</span>
        </div>

        {formOpen && (
          <form className="pr-form" onSubmit={handleSubmit}>
            <input
              className="pr-input"
              type="text"
              placeholder="github.com/owner/repo/pull/123"
              autoComplete="off"
              spellCheck={false}
              value={input}
              ref={inputRef}
              onChange={(e) => onInput(e.target.value)}
            />
            <button type="submit">Review</button>
          </form>
        )}

        <div className="topbar-actions">
          {started && !wide && !editing && (
            <button
              type="button"
              className="tool"
              title="Load a different PR"
              onClick={openEditor}
            >
              <span aria-hidden="true">⌕</span>
              <span className="sr-only">Load a different PR</span>
            </button>
          )}
          {started && (
            <button
              className="tool"
              type="button"
              aria-expanded={drawerOpen}
              id="files-btn"
              onClick={onFilesToggle}
            >
              Files
            </button>
          )}
          {wide ? (
            <DisplayControls
              prefs={prefs}
              hasToken={hasToken}
              onToken={onToken}
              variant="bar"
            />
          ) : (
            <SettingsMenu
              prefs={prefs}
              hasToken={hasToken}
              onToken={onToken}
              onOpenChange={onSettingsOpenChange}
            />
          )}
        </div>
      </div>
    </header>
  );
}

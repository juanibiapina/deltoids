import { useEffect, useId, useRef, useState } from "react";
import type { Prefs } from "../hooks/usePrefs";
import { DisplayControls } from "./DisplayControls";

interface SettingsMenuProps {
  prefs: Prefs;
  hasToken: boolean;
  onToken: () => void;
  // Reported so the collapse hook keeps the header shown while the menu is open.
  onOpenChange?: (open: boolean) => void;
}

// Narrow-screen home for the display controls: one trigger opens a popover so
// the header stays a single row. Trigger carries aria-haspopup/aria-expanded;
// the panel closes on outside click or Escape and returns focus to the trigger.
export function SettingsMenu({
  prefs,
  hasToken,
  onToken,
  onOpenChange,
}: SettingsMenuProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();

  useEffect(() => {
    onOpenChange?.(open);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="settings" ref={rootRef}>
      <button
        type="button"
        className="tool settings-btn"
        ref={triggerRef}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={panelId}
        title="Display settings"
        onClick={() => setOpen((v) => !v)}
      >
        <span aria-hidden="true">⚙</span>
        <span className="sr-only">Settings</span>
      </button>
      {open && (
        <div
          className="settings-panel"
          id={panelId}
          role="dialog"
          aria-label="Display settings"
        >
          <DisplayControls
            prefs={prefs}
            hasToken={hasToken}
            onToken={onToken}
            variant="menu"
            selectId={`${panelId}-theme`}
          />
        </div>
      )}
    </div>
  );
}

// Drag a bottom sheet down to dismiss it. A drag starts only when the sheet's
// list is scrolled to the top, so a downward swipe inside the list scrolls it
// until it reaches the top. Past a quarter of the sheet's height, or with a
// quick flick, the sheet closes; otherwise it springs back.

const FLICK_PX_PER_MS = 0.5;

export function attachSheetDrag(sheet: HTMLElement, onClose: () => void): () => void {
  let startY = 0;
  let startTime = 0;
  let dy = 0;
  let dragging = false;

  const onStart = (e: TouchEvent) => {
    if (e.touches.length !== 1 || sheet.scrollTop > 0) return;
    startY = e.touches[0].clientY;
    startTime = e.timeStamp;
    dy = 0;
    dragging = true;
  };
  const onMove = (e: TouchEvent) => {
    if (!dragging) return;
    dy = e.touches[0].clientY - startY;
    if (dy <= 0) {
      sheet.style.transform = "";
      return;
    }
    e.preventDefault();
    sheet.style.transition = "none";
    sheet.style.transform = `translateY(${dy}px)`;
  };
  const onEnd = (e: TouchEvent) => {
    if (!dragging) return;
    dragging = false;
    sheet.style.transition = "";
    sheet.style.transform = "";
    const speed = dy / Math.max(1, e.timeStamp - startTime);
    if (dy > sheet.clientHeight / 4 || (dy > 24 && speed > FLICK_PX_PER_MS)) onClose();
  };

  sheet.addEventListener("touchstart", onStart, { passive: true });
  sheet.addEventListener("touchmove", onMove, { passive: false });
  sheet.addEventListener("touchend", onEnd);
  sheet.addEventListener("touchcancel", onEnd);
  return () => {
    sheet.removeEventListener("touchstart", onStart);
    sheet.removeEventListener("touchmove", onMove);
    sheet.removeEventListener("touchend", onEnd);
    sheet.removeEventListener("touchcancel", onEnd);
  };
}

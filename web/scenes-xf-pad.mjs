/**
 * Shared 2D crossfader pad pointer handling for scenes and remote pages.
 */

export function clamp(v, lo, hi) {
  return Math.min(hi, Math.max(lo, v));
}

export function posFromPointer(pad, clientX, clientY) {
  const rect = pad.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return { x: 0.5, y: 0.5 };
  return {
    x: clamp((clientX - rect.left) / rect.width, 0, 1),
    y: clamp((clientY - rect.top) / rect.height, 0, 1),
  };
}

export function updatePadHandle(pad, handle, x, y) {
  if (!pad || !handle) return;
  handle.style.left = `${x * 100}%`;
  handle.style.top = `${y * 100}%`;
}

/**
 * @param {HTMLElement} pad
 * @param {HTMLElement} handle
 * @param {object} opts
 * @param {() => {x:number,y:number}} opts.getPos
 * @param {(x:number,y:number) => void} opts.setPos
 * @param {() => void} [opts.onChange]
 * @param {() => void} [opts.onGrabStart]
 * @param {() => void} [opts.onGrabEnd]
 */
export function bindXfPad(pad, handle, opts) {
  if (!pad) return () => {};

  let dragging = false;

  function applyFromEvent(ev) {
    const { x, y } = posFromPointer(pad, ev.clientX, ev.clientY);
    opts.setPos(x, y);
    updatePadHandle(pad, handle, x, y);
    opts.onChange?.();
  }

  function onPointerDown(ev) {
    if (ev.button !== 0 && ev.pointerType === "mouse") return;
    dragging = true;
    pad.setPointerCapture(ev.pointerId);
    opts.onGrabStart?.();
    applyFromEvent(ev);
    ev.preventDefault();
  }

  function onPointerMove(ev) {
    if (!dragging) return;
    applyFromEvent(ev);
  }

  function onPointerUp(ev) {
    if (!dragging) return;
    dragging = false;
    try {
      pad.releasePointerCapture(ev.pointerId);
    } catch (_) {}
    opts.onGrabEnd?.();
  }

  pad.addEventListener("pointerdown", onPointerDown);
  pad.addEventListener("pointermove", onPointerMove);
  pad.addEventListener("pointerup", onPointerUp);
  pad.addEventListener("pointercancel", onPointerUp);

  const { x, y } = opts.getPos();
  updatePadHandle(pad, handle, x, y);

  return () => {
    pad.removeEventListener("pointerdown", onPointerDown);
    pad.removeEventListener("pointermove", onPointerMove);
    pad.removeEventListener("pointerup", onPointerUp);
    pad.removeEventListener("pointercancel", onPointerUp);
  };
}

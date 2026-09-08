import { type RefObject, useEffect, useRef } from 'react';

export interface DraggableOptions {
  /** localStorage key the position is saved under and restored from. */
  storageKey: string;
  /** Selector, relative to the element, for the part that starts a drag. Buttons inside it are excluded. */
  handleSelector: string;
}

export interface DraggableHandle {
  /** Snaps the element back to its natural (CSS) position and forgets the saved one. */
  reset: () => void;
}

type Point = { x: number; y: number };

/** Clamps a candidate top-left corner so a `width` x `height` box stays inside the viewport. */
function clampPoint(width: number, height: number, x: number, y: number): Point {
  return {
    x: Math.min(Math.max(x, 0), Math.max(0, window.innerWidth - width)),
    y: Math.min(Math.max(y, 0), Math.max(0, window.innerHeight - height)),
  };
}

/**
 * Makes `ref`'s element draggable by a handle inside it, with pointer events (not the HTML5 drag
 * API, so touch works too). Undragged, the element is left entirely alone -- it keeps whatever
 * `right`/`bottom`-anchored, content-sized position its own CSS gives it. The first drag (or a
 * restored position from a previous visit) switches it to explicit `left`/`top` pixels and turns
 * off `right`/`bottom`, which is what makes the position survive later content-height changes
 * (the walkthrough card's body is a different length on every step): a `right`/`bottom` anchor
 * recomputes the box's top-left corner from its *current* height on every reflow, so a fixed
 * `transform` layered on top of that anchor would drift by exactly as much as the height changes
 * between steps. Explicit `left`/`top` has no such dependency.
 *
 * `pointerdown` is delegated to `document` and re-reads `ref.current` on every event, rather than
 * being bound once to the handle found at effect-setup time: the dashboard this overlay sits in
 * re-renders its shell for reasons that have nothing to do with the walkthrough, and a listener
 * bound to a since-replaced DOM node goes quiet with no error -- a much worse failure than the
 * extra `closest()` call this costs on every pointerdown.
 */
export function useDraggable<T extends HTMLElement>(ref: RefObject<T | null>, { storageKey, handleSelector }: DraggableOptions): DraggableHandle {
  const pinned = useRef<Point | null>(null); // the absolute {left, top} once pinned; null = still on its natural CSS position
  const resetRef = useRef<() => void>(() => {});

  useEffect(() => {
    const pin = (el: T, p: Point) => {
      const r = el.getBoundingClientRect();
      const c = clampPoint(r.width, r.height, p.x, p.y);
      pinned.current = c;
      el.style.left = `${c.x}px`;
      el.style.top = `${c.y}px`;
      el.style.right = 'auto';
      el.style.bottom = 'auto';
    };
    const unpin = (el: T) => {
      pinned.current = null;
      el.style.left = '';
      el.style.top = '';
      el.style.right = '';
      el.style.bottom = '';
    };
    const save = (p: Point) => {
      try {
        localStorage.setItem(storageKey, JSON.stringify(p));
      } catch {
        // no localStorage (private mode, disabled, quota) -- the position just does not survive reload
      }
    };

    // Restore a saved position, if there is one and it parses; otherwise leave the element on its
    // natural CSS position untouched (covers both "never dragged" and "localStorage threw").
    const el0 = ref.current;
    if (el0) {
      try {
        const raw = localStorage.getItem(storageKey);
        if (raw) pin(el0, JSON.parse(raw) as Point);
      } catch {
        // unavailable, disabled, or corrupt -- stay on the natural position
      }
    }

    let finishDrag: (() => void) | null = null;

    const onPointerDown = (e: PointerEvent) => {
      const el = ref.current;
      const target = e.target as HTMLElement;
      const handle = target.closest<HTMLElement>(handleSelector);
      if (!el || !handle || !el.contains(handle) || target.closest('button')) return; // not our handle, or the reset affordance inside it
      const pointerId = e.pointerId;
      const startX = e.clientX;
      const startY = e.clientY;
      const startRect = el.getBoundingClientRect(); // the drag's base, whether the box was already pinned or still on its natural spot
      try {
        handle.setPointerCapture(pointerId);
      } catch {
        // best effort; the window listeners below do the real work regardless
      }
      handle.classList.add('dragging');

      const onPointerMove = (m: PointerEvent) => {
        if (m.pointerId !== pointerId || !ref.current) return;
        pin(ref.current, { x: startRect.left + (m.clientX - startX), y: startRect.top + (m.clientY - startY) });
      };
      finishDrag = () => {
        window.removeEventListener('pointermove', onPointerMove);
        window.removeEventListener('pointerup', onPointerUp);
        window.removeEventListener('pointercancel', onPointerUp);
        try {
          handle.releasePointerCapture(pointerId);
        } catch {
          // already released (e.g. pointercancel got there first)
        }
        handle.classList.remove('dragging');
        finishDrag = null;
        if (pinned.current) save(pinned.current);
      };
      const onPointerUp = (u: PointerEvent) => {
        if (u.pointerId !== pointerId) return;
        finishDrag?.();
      };
      window.addEventListener('pointermove', onPointerMove);
      window.addEventListener('pointerup', onPointerUp);
      window.addEventListener('pointercancel', onPointerUp);
      e.preventDefault();
    };
    const onResize = () => {
      if (ref.current && pinned.current) pin(ref.current, pinned.current);
    };

    document.addEventListener('pointerdown', onPointerDown);
    window.addEventListener('resize', onResize);

    resetRef.current = () => {
      if (ref.current) unpin(ref.current);
      try {
        localStorage.removeItem(storageKey);
      } catch {
        // nothing saved to begin with, or no localStorage -- either way there is nothing to remove
      }
    };

    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('resize', onResize);
      finishDrag?.(); // a drag in flight when this unmounts still cleans up its window listeners
      resetRef.current = () => {};
    };
  }, [ref, storageKey, handleSelector]);

  return { reset: () => resetRef.current() };
}

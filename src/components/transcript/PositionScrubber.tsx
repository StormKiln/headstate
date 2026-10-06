import { useRef, useState } from "react";

/** A local draft during gestures; the host resolves only committed percentages. */
export function PositionScrubber({ seek }: { seek: (percent: number) => void }) {
  const [percent, setPercent] = useState(100);
  const gesture = useRef<"pointer" | "keyboard" | null>(null);
  const committed = useRef<number | null>(null);
  const beforeGesture = useRef(100);
  const commit = (value: number) => {
    gesture.current = null;
    if (committed.current === value) return;
    committed.current = value;
    seek(value);
  };
  return (
    <label className="inline-flex items-center gap-2">
      Transcript position
      <input
        type="range" min={0} max={100} step={1} value={percent}
        aria-label="Transcript position"
        aria-valuetext={`About ${percent}% through transcript`}
        onPointerDown={(e) => {
          beforeGesture.current = percent;
          gesture.current = "pointer";
          committed.current = null;
          e.currentTarget.setPointerCapture?.(e.pointerId);
        }}
        onPointerUp={() => commit(percent)}
        onPointerCancel={() => { gesture.current = null; setPercent(beforeGesture.current); }}
        onKeyDown={(e) => {
          if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", "Enter"].includes(e.key)) {
            if (gesture.current === null) committed.current = null;
            gesture.current = "keyboard";
          }
        }}
        onKeyUp={() => { if (gesture.current === "keyboard") commit(percent); }}
        onBlur={() => { if (gesture.current !== null) commit(percent); }}
        onChange={(e) => {
          const value = Number(e.currentTarget.value);
          setPercent(value);
          // Assistive controls may emit change without pointer or keyboard events.
          if (gesture.current === null) commit(value);
        }}
      />
    </label>
  );
}

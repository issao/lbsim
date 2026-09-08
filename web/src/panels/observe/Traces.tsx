// The engine records no request traces yet (U104 wires GetTraces); the tab stays so it has a
// place, and says so rather than drawing a list synthesized in the browser.
export function Traces() {
  return (
    <p className="note" style={{ margin: 0 }} id="trace-list">
      traces: not simulated yet
    </p>
  );
}

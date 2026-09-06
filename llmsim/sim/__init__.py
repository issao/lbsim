"""Generic discrete-event simulation machinery.

Nothing in this package knows anything about LLM inference. Keep it that way:
the domain model lives in `llmsim.serving`, policies in `llmsim.policy`.
"""

from llmsim.sim.engine import Event, Interrupt, Process, Simulator
from llmsim.sim.metrics import Recorder, percentile, summarize
from llmsim.sim.rng import RngRegistry

__all__ = [
    "Event",
    "Interrupt",
    "Process",
    "Recorder",
    "RngRegistry",
    "Simulator",
    "percentile",
    "summarize",
]

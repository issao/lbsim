"""Named, independently seeded random streams.

Why a registry instead of one global generator: if arrivals, prompt lengths and
failure injection all draw from one stream, then changing the arrival rate
changes every other draw too. Comparing two policies would then be comparing two
different workloads, and any difference you measure is partly noise.

With named streams, ``rng.stream("arrival")`` is unaffected by how many draws
``rng.stream("prompt_len")`` has taken. Turning on failure injection does not
perturb the workload. That is what makes A/B comparison in a simulator valid.

Stream seeds derive from ``blake2b(name, key=master_seed)``, not from Python's
``hash()``, which is randomized per process for strings and would destroy
reproducibility across runs.
"""

from __future__ import annotations

import hashlib
import math
import random

_PERSON = b"lbsim.rng"


class RngRegistry:
    """Creates and caches one :class:`random.Random` per name."""

    def __init__(self, master_seed: int) -> None:
        self.master_seed = int(master_seed)
        self._key = self.master_seed.to_bytes(8, "little", signed=False)
        self._streams: dict[str, random.Random] = {}

    def derive_seed(self, name: str) -> int:
        digest = hashlib.blake2b(
            name.encode("utf-8"), digest_size=8, key=self._key, person=_PERSON
        ).digest()
        return int.from_bytes(digest, "little")

    def stream(self, name: str) -> random.Random:
        rng = self._streams.get(name)
        if rng is None:
            rng = random.Random(self.derive_seed(name))
            self._streams[name] = rng
        return rng

    def child(self, name: str) -> "RngRegistry":
        """A sub-registry, so per-replica or per-tenant streams stay independent."""
        return RngRegistry(self.derive_seed(name))

    def __repr__(self) -> str:
        return f"RngRegistry(master_seed={self.master_seed})"


# ---------------------------------------------------------------------------
# Distributions
#
# Parameterised the way a workload is actually described: by a mean and a
# spread you can read off a dashboard, not by the shape parameters of the
# underlying math.
# ---------------------------------------------------------------------------


def exponential(rng: random.Random, mean: float) -> float:
    """Interarrival time of a Poisson process with rate ``1/mean``."""
    if mean <= 0:
        return 0.0
    return rng.expovariate(1.0 / mean)


def lognormal(rng: random.Random, mean: float, cv: float) -> float:
    """Lognormal with the given arithmetic ``mean`` and coefficient of variation.

    ``cv`` is stddev/mean. Token counts and service times are strongly
    right-skewed in practice, so lognormal is the sane default; ``cv`` around
    1.0 to 2.0 matches published prompt and completion length distributions
    better than an exponential does.
    """
    if mean <= 0:
        return 0.0
    if cv <= 0:
        return mean
    sigma2 = math.log1p(cv * cv)
    sigma = math.sqrt(sigma2)
    mu = math.log(mean) - sigma2 / 2.0
    return math.exp(rng.normalvariate(mu, sigma))


def pareto(rng: random.Random, scale: float, alpha: float) -> float:
    """Heavy tail. ``alpha`` <= 2 gives infinite variance, <= 1 infinite mean.

    Use for the long-context tail: a small fraction of requests with enormous
    prompts is exactly the load pattern that breaks KV-cache capacity planning.
    """
    return scale * (rng.paretovariate(alpha))


def bounded_int(value: float, lo: int, hi: int) -> int:
    """Clamp and round a continuous draw into a token count."""
    return max(lo, min(hi, int(round(value))))


def zipf(rng: random.Random, n: int, s: float = 1.0) -> int:
    """Draw in ``[0, n)`` with Zipf-like popularity, index 0 most popular.

    Models skewed key popularity: which system prompt, which tenant, which
    cached prefix. Real traffic is very skewed and uniform choice will
    understate prefix-cache hit rates badly.
    """
    if n <= 1:
        return 0
    weights = [1.0 / ((i + 1) ** s) for i in range(n)]
    total = sum(weights)
    target = rng.random() * total
    acc = 0.0
    for i, w in enumerate(weights):
        acc += w
        if acc >= target:
            return i
    return n - 1


def sample_bimodal(
    rng: random.Random, p_long: float, short_mean: float, long_mean: float, cv: float
) -> float:
    """Mixture of a short and a long mode.

    Real inference traffic is a mixture, not a single distribution: interactive
    chat plus long-document work in the same queue. The mixture is what creates
    head-of-line blocking, so a unimodal workload will hide the problem you are
    trying to study.
    """
    mean = long_mean if rng.random() < p_long else short_mean
    return lognormal(rng, mean, cv)

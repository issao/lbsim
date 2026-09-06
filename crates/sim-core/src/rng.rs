//! Named, independently seeded random streams.
//!
//! Why named rather than one global generator: if arrivals, request sizes and failure injection all
//! draw from one stream, then changing the arrival rate shifts every other draw too, and an A/B
//! comparison silently compares two different workloads. With named streams, enabling failure
//! injection cannot perturb the workload.
//!
//! Stream seeds come from a fixed hash of the name mixed with the master seed, so they are stable
//! across processes and platforms.

/// xoshiro256++ over a splitmix64-expanded seed. Small, fast, and deterministic everywhere.
#[derive(Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn from_seed(seed: u64) -> Self {
        let mut z = seed;
        let mut s = [0u64; 4];
        for slot in s.iter_mut() {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            *slot = x ^ (x >> 31);
        }
        Rng { s }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[0]
            .wrapping_add(self.s[3])
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn f64(&mut self) -> f64 {
        // 53 significant bits, the most an f64 can hold exactly.
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }

    /// Exponential with the given mean. Interarrival time of a Poisson process.
    pub fn exponential(&mut self, mean: f64) -> f64 {
        if mean <= 0.0 {
            return 0.0;
        }
        // 1 - u avoids ln(0).
        -mean * (1.0 - self.f64()).ln()
    }

    fn normal(&mut self) -> f64 {
        // Box-Muller. One of the pair is discarded, which costs nothing that matters here and
        // avoids carrying state that would complicate reproducibility.
        let u1 = 1.0 - self.f64();
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Lognormal with the given arithmetic mean and coefficient of variation.
    ///
    /// Parameterised by mean and CV rather than by the underlying mu and sigma, because those are
    /// the numbers a workload is actually described by. Token counts are strongly right-skewed in
    /// practice, and `docs/calibration.md` measures output CVs above 3.
    pub fn lognormal(&mut self, mean: f64, cv: f64) -> f64 {
        if mean <= 0.0 {
            return 0.0;
        }
        if cv <= 0.0 {
            return mean;
        }
        let sigma2 = (1.0 + cv * cv).ln();
        let sigma = sigma2.sqrt();
        let mu = mean.ln() - sigma2 / 2.0;
        (mu + sigma * self.normal()).exp()
    }
}

/// One `Rng` per name, derived from a master seed.
pub struct Streams {
    master: u64,
}

impl Streams {
    pub fn new(master: u64) -> Self {
        Streams { master }
    }

    /// FNV-1a over the name, mixed with the master seed. Stable across processes, unlike a
    /// language-provided string hash, which would make runs irreproducible.
    pub fn stream(&self, name: &str) -> Rng {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        Rng::from_seed(h ^ self.master.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// A per-entity stream, so replica 3's failures are independent of replica 4's.
    pub fn stream_indexed(&self, name: &str, index: u64) -> Rng {
        let mut r = self.stream(name);
        Rng::from_seed(r.next_u64() ^ index.wrapping_mul(0xD6E8_FEB8_6659_FD93))
    }
}

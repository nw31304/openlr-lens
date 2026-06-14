/// A bearing interval in degrees, interpreted mod 360. Use only for bearings.
/// Containment and overlap must handle wraparound (e.g. 350°–10°).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CircularInterval {
    pub lb_deg: f64,
    pub ub_deg: f64,
}

impl CircularInterval {
    pub fn point(deg: f64) -> Self {
        Self { lb_deg: deg, ub_deg: deg }
    }

    /// Widen by `tolerance` degrees on each side.
    pub fn widen(self, tolerance: f64) -> Self {
        Self { lb_deg: self.lb_deg - tolerance, ub_deg: self.ub_deg + tolerance }
    }

    /// True if `deg` falls within this interval (mod 360, inclusive).
    pub fn contains(self, deg: f64) -> bool {
        let span = (self.ub_deg - self.lb_deg).rem_euclid(360.0);
        let offset = (deg - self.lb_deg).rem_euclid(360.0);
        offset <= span
    }

    /// Signed shortest angular distance from `deg` to the nearest bound.
    /// Returns 0.0 if `deg` is inside the interval.
    pub fn excess(self, deg: f64) -> f64 {
        if self.contains(deg) {
            return 0.0;
        }
        let to_lb = (self.lb_deg - deg).rem_euclid(360.0);
        let to_ub = (deg - self.ub_deg).rem_euclid(360.0);
        to_lb.min(to_ub)
    }
}

/// A linear (non-circular) interval in meters. Use for DNP, offsets, and distances.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LinearInterval {
    pub lb: f64,
    pub ub: f64,
}

impl LinearInterval {
    pub fn point(v: f64) -> Self {
        Self { lb: v, ub: v }
    }

    pub fn widen(self, tolerance: f64) -> Self {
        Self { lb: self.lb - tolerance, ub: self.ub + tolerance }
    }

    pub fn contains(self, v: f64) -> bool {
        v >= self.lb && v <= self.ub
    }

    /// Distance outside the interval; 0.0 if inside.
    pub fn excess(self, v: f64) -> f64 {
        if v < self.lb { self.lb - v } else if v > self.ub { v - self.ub } else { 0.0 }
    }
}

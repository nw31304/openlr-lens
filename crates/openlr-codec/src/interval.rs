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
        // A span of exactly 360° (or wider after widen()) is the full circle.
        // rem_euclid(360) maps 360.0 → 0.0, so we must check before taking the remainder.
        if self.ub_deg - self.lb_deg >= 360.0 {
            return true;
        }
        let span = (self.ub_deg - self.lb_deg).rem_euclid(360.0);
        let offset = (deg - self.lb_deg).rem_euclid(360.0);
        offset <= span
    }

    /// Shortest angular distance from `deg` to the nearest bound.
    /// Returns 0.0 if `deg` is inside the interval.
    pub fn excess(self, deg: f64) -> f64 {
        if self.ub_deg - self.lb_deg >= 360.0 {
            return 0.0;
        }
        if self.contains(deg) {
            return 0.0;
        }
        let to_lb = (self.lb_deg - deg).rem_euclid(360.0);
        let to_ub = (deg - self.ub_deg).rem_euclid(360.0);
        to_lb.min(to_ub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_circle_contains_everything() {
        let full = CircularInterval { lb_deg: 0.0, ub_deg: 360.0 };
        for deg in [0.0, 90.0, 180.0, 270.0, 359.99] {
            assert!(full.contains(deg), "{deg}° should be inside [0,360]");
            assert_eq!(full.excess(deg), 0.0);
        }
    }

    #[test]
    fn over_360_after_widen_contains_everything() {
        // A narrow bucket widened by a large tolerance exceeds 360° — must still be full circle.
        let narrow = CircularInterval { lb_deg: 90.0, ub_deg: 101.25 };
        let wide = narrow.widen(180.0); // span = 361.25°
        assert!(wide.contains(0.0));
        assert!(wide.contains(270.0));
        assert_eq!(wide.excess(270.0), 0.0);
    }

    #[test]
    fn exactly_360_not_zero() {
        // The original bug: span = 360 → rem_euclid(360) = 0 → only lb_deg matched.
        let full = CircularInterval { lb_deg: 45.0, ub_deg: 405.0 }; // 360° span, offset lb
        assert!(full.contains(0.0), "0° should be inside a full-circle interval");
        assert!(full.contains(200.0));
    }

    #[test]
    fn wraparound_interval() {
        let wrap = CircularInterval { lb_deg: 350.0, ub_deg: 10.0 };
        assert!(wrap.contains(0.0));
        assert!(wrap.contains(355.0));
        assert!(!wrap.contains(180.0));
        assert!(wrap.excess(180.0) > 0.0);
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

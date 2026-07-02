use crate::{CircularInterval, LinearInterval};

/// A Location Reference Point in the unified interval model.
/// Format-specific bit-unpacking (v3 / TPEG) lives in the decoder modules;
/// everything downstream of the decoder is format-agnostic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lrp {
    /// WGS84 (longitude, latitude).
    pub coord: (f64, f64),
    /// Forward bearing from the projection point (or backward for the last LRP).
    pub bearing: CircularInterval,
    pub frc: u8,
    pub fow: u8,
    /// Lowest FRC permitted on the path to the next LRP. None on the last LRP.
    pub lfrcnp: Option<u8>,
    /// Distance to next point in meters. None on the last LRP.
    pub dnp: Option<LinearInterval>,
    /// Positive offset interval (meters from start of first edge). None if absent.
    /// For v3: computed in the engine once total path length is known (see pos_offset_raw).
    /// For TPEG: set directly by the decoder (exact value, lb == ub).
    pub pos_offset: Option<LinearInterval>,
    /// Negative offset interval (meters from end of last edge). None if absent.
    pub neg_offset: Option<LinearInterval>,
    /// Raw v3 positive offset byte (0–255). Present only for v3 line/PAL locations.
    /// The engine derives pos_offset from this once total path length is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos_offset_raw: Option<u8>,
    /// Raw v3 negative offset byte (0–255). Present only for v3 line locations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neg_offset_raw: Option<u8>,
}

/// Orientation attribute for PointAlongLine — direction of travel at the encoded point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Orientation {
    /// No orientation information / not applicable.
    NoOrientation = 0,
    /// Travel from first LRP toward second LRP.
    FirstTowardSecond = 1,
    /// Travel from second LRP toward first LRP.
    SecondTowardFirst = 2,
    /// Both directions.
    BothDirections = 3,
}

impl Orientation {
    pub fn from_u8(v: u8) -> Self {
        match v & 0x03 {
            0 => Orientation::NoOrientation,
            1 => Orientation::FirstTowardSecond,
            2 => Orientation::SecondTowardFirst,
            _ => Orientation::BothDirections,
        }
    }
}

/// Side-of-road attribute for PointAlongLine — which side of the road the point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SideOfRoad {
    /// Directly on the road or not applicable.
    DirectlyOnOrNA = 0,
    /// Right side of the road in the direction of travel.
    Right = 1,
    /// Left side of the road in the direction of travel.
    Left = 2,
    /// Both sides.
    Both = 3,
}

impl SideOfRoad {
    pub fn from_u8(v: u8) -> Self {
        match v & 0x03 {
            0 => SideOfRoad::DirectlyOnOrNA,
            1 => SideOfRoad::Right,
            2 => SideOfRoad::Left,
            _ => SideOfRoad::Both,
        }
    }
}

/// A fully-parsed OpenLR location reference.
///
/// Network-based types (Line, ClosedLine, PAL, POI) carry LRPs for map-matching.
/// Geometry types (GeoCoordinate, Circle, Rectangle, Grid, Polygon) carry only
/// the shape — no map-matching is needed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "location_type")]
pub enum LocationReference {
    Line { lrps: Vec<Lrp> },
    ClosedLine { lrps: Vec<Lrp> },
    PointAlongLine { lrps: Vec<Lrp>, orientation: Orientation, side_of_road: SideOfRoad },
    PoiWithAccessPoint { lrps: Vec<Lrp>, orientation: Orientation, side_of_road: SideOfRoad, poi: (f64, f64) },
    GeoCoordinate { coord: (f64, f64) },
    Circle { center: (f64, f64), radius_m: u32 },
    Rectangle { lower_left: (f64, f64), upper_right: (f64, f64) },
    Grid { lower_left: (f64, f64), upper_right: (f64, f64), n_cols: u16, n_rows: u16 },
    Polygon { coords: Vec<(f64, f64)> },
}

impl LocationReference {
    /// Return the LRP slice for network-based location types; `None` for geometry types.
    pub fn lrps(&self) -> Option<&[Lrp]> {
        match self {
            Self::Line { lrps }
            | Self::ClosedLine { lrps }
            | Self::PointAlongLine { lrps, .. }
            | Self::PoiWithAccessPoint { lrps, .. } => Some(lrps.as_slice()),
            _ => None,
        }
    }

    /// True for PointAlongLine and PoiWithAccessPoint.
    pub fn is_point_on_line(&self) -> bool {
        matches!(self, Self::PointAlongLine { .. } | Self::PoiWithAccessPoint { .. })
    }

    /// String tag matching the serde `location_type` discriminant.
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Line { .. }               => "Line",
            Self::ClosedLine { .. }         => "ClosedLine",
            Self::PointAlongLine { .. }     => "PointAlongLine",
            Self::PoiWithAccessPoint { .. } => "PoiWithAccessPoint",
            Self::GeoCoordinate { .. }      => "GeoCoordinate",
            Self::Circle { .. }             => "Circle",
            Self::Rectangle { .. }          => "Rectangle",
            Self::Grid { .. }               => "Grid",
            Self::Polygon { .. }            => "Polygon",
        }
    }
}

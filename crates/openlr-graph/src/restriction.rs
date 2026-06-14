use crate::{NodeId, SegmentId};

#[derive(Debug, Clone)]
pub struct TurnRestriction {
    pub from_seg: SegmentId,
    pub via_node: NodeId,
    pub to_seg: SegmentId,
}

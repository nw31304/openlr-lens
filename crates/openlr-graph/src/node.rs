#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone)]
pub struct NetworkNode {
    pub id: NodeId,
    pub lon: f64,
    pub lat: f64,
    pub gers_id: [u8; 16],
    pub is_boundary: bool,
}

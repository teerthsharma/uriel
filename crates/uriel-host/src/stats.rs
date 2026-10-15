#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostStats {
    pub arena_cap: u64,
    pub arena_used: u64,
    pub epoch: u64,
    pub requests_served: u64,
    pub requests_failed: u64,
    pub requests_busy: u64,
    pub in_flight: u64,
    pub queue_len: u64,
    pub connections: u64,
    pub chunks: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferGauge {
    pub inbound_capacity: usize,
    pub outbound_capacity: usize,
}

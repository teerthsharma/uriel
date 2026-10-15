#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub id: RegionId,
    pub size: u64,
    pub community: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
    Scan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Normal,
    Elevated,
    High,
    Critical,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Normal, Level::Elevated, Level::High, Level::Critical];

    pub fn eighths(self) -> u64 {
        match self {
            Level::Normal => 8,
            Level::Elevated => 6,
            Level::High => 4,
            Level::Critical => 1,
        }
    }

    pub(crate) fn tag(self) -> u8 {
        match self {
            Level::Normal => 0,
            Level::Elevated => 1,
            Level::High => 2,
            Level::Critical => 3,
        }
    }

    pub(crate) fn from_tag(t: u8) -> Option<Level> {
        Level::ALL.get(usize::from(t)).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Access { region: RegionId, kind: AccessKind },
    Pin { region: RegionId },
    Unpin { region: RegionId },
    Resize { region: RegionId, size: u64 },
    Pressure { level: Level },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

impl Tier {
    pub(crate) fn tag(self) -> u8 {
        match self {
            Tier::Hot => 0,
            Tier::Warm => 1,
            Tier::Cold => 2,
        }
    }

    pub(crate) fn from_tag(t: u8) -> Option<Tier> {
        match t {
            0 => Some(Tier::Hot),
            1 => Some(Tier::Warm),
            2 => Some(Tier::Cold),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lifetime {
    Ephemeral,
    Session,
    Durable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    BadCheckpoint(&'static str),
    ConfigMismatch,
}

impl core::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PlaceError::BadCheckpoint(m) => write!(f, "bad checkpoint: {m}"),
            PlaceError::ConfigMismatch => {
                write!(f, "checkpoint was taken under a different config")
            }
        }
    }
}

impl std::error::Error for PlaceError {}

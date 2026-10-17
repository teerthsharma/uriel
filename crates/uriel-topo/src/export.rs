use crate::error::{Error, Result};
use crate::schedule::Schedule;

const MAGIC: [u8; 4] = *b"UCSR";
const VERSION: u32 = 1;
const HEADER_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCsr {
    block_size: u32,
    offsets: Vec<i32>,
    indices: Vec<i32>,
}

fn malformed<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Malformed(msg.into()))
}

fn check_block_size(block_size: u32) -> Result<()> {
    if block_size == 0 || !block_size.is_power_of_two() {
        return Err(Error::InvalidParameter(format!(
            "block size {block_size} is not a positive power of two"
        )));
    }
    Ok(())
}

impl KernelCsr {
    pub fn new(block_size: u32, offsets: Vec<i32>, indices: Vec<i32>) -> Result<Self> {
        check_block_size(block_size)?;
        if offsets.len() < 2 {
            return malformed("at least one block is required");
        }
        if offsets[0] != 0 {
            return malformed("first offset must be 0");
        }
        if offsets.windows(2).any(|w| w[0] > w[1]) {
            return malformed("offsets must not decrease");
        }
        if *offsets.last().unwrap_or(&-1) as i64 != indices.len() as i64 {
            return malformed("last offset must equal the number of indices");
        }
        for (i, w) in offsets.windows(2).enumerate() {
            let row = &indices[w[0] as usize..w[1] as usize];
            let (Some(&first), Some(&last)) = (row.first(), row.last()) else {
                return malformed(format!("row {i} is empty"));
            };
            if first < 0 || row.windows(2).any(|p| p[0] >= p[1]) {
                return malformed(format!(
                    "row {i} is not strictly ascending and non-negative"
                ));
            }
            if last as i64 != i as i64 {
                return malformed(format!("row {i} must end on its diagonal block"));
            }
        }
        Ok(Self {
            block_size,
            offsets,
            indices,
        })
    }

    pub fn from_schedule(s: &Schedule, block_size: u32) -> Result<Self> {
        check_block_size(block_size)?;
        if s.indptr.first() != Some(&0)
            || s.indptr.windows(2).any(|w| w[0] > w[1])
            || s.indptr.last() != Some(&s.indices.len())
        {
            return malformed("schedule indptr does not describe its indices");
        }
        let to_i32 = |x: usize| {
            i32::try_from(x).map_err(|_| Error::OutOfRange(format!("{x} does not fit in i32")))
        };
        let mut offsets = Vec::with_capacity(s.indptr.len());
        let mut indices = Vec::with_capacity(s.indices.len() + s.rows());
        offsets.push(0);
        for i in 0..s.rows() {
            let row = s.row(i);
            for &j in row {
                indices.push(to_i32(j)?);
            }
            if row.last() != Some(&i) {
                indices.push(to_i32(i)?);
            }
            offsets.push(to_i32(indices.len())?);
        }
        Self::new(block_size, offsets, indices)
    }

    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    pub fn blocks(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn offsets(&self) -> &[i32] {
        &self.offsets
    }

    pub fn indices(&self) -> &[i32] {
        &self.indices
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(HEADER_LEN + 4 * (self.offsets.len() + self.indices.len()));
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.block_size.to_le_bytes());
        out.extend_from_slice(&(self.blocks() as u32).to_le_bytes());
        out.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        for v in self.offsets.iter().chain(&self.indices) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return malformed(format!("{} bytes is shorter than the header", bytes.len()));
        }
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if bytes[..4] != MAGIC {
            return malformed("bad magic");
        }
        if word(4) != VERSION {
            return malformed(format!("unsupported version {}", word(4)));
        }
        let (block_size, blocks, nnz) = (word(8), word(12) as u64, word(16) as u64);
        let expected = HEADER_LEN as u64 + 4 * (blocks + 1) + 4 * nnz;
        if expected != bytes.len() as u64 {
            return malformed(format!(
                "header implies {expected} bytes, got {}",
                bytes.len()
            ));
        }
        let mut values = bytes[HEADER_LEN..]
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        let offsets: Vec<i32> = values.by_ref().take(blocks as usize + 1).collect();
        let indices: Vec<i32> = values.collect();
        Self::new(block_size, offsets, indices)
    }
}

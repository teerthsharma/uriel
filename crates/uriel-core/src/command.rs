use std::collections::BTreeSet;

use crate::catalog::ConstraintKind;
use crate::checksum::crc32c;
use crate::codec::{put_props, put_str, put_u32, Reader};
use crate::error::{Error, Result};
use crate::ids::{NodeId, Props, RelId};

const BATCH_VERSION: u8 = 1;
const MIN_COMMAND_BYTES: usize = 9;
const HEADER_BYTES: usize = 1 + 8 + 4;
const CRC_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    CreateNode {
        id: NodeId,
        labels: BTreeSet<String>,
        props: Props,
    },
    SetNodeProps {
        id: NodeId,
        props: Props,
    },
    AddLabel {
        id: NodeId,
        label: String,
    },
    RemoveLabel {
        id: NodeId,
        label: String,
    },
    DeleteNode {
        id: NodeId,
    },
    CreateRel {
        id: RelId,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        props: Props,
    },
    SetRelProps {
        id: RelId,
        props: Props,
    },
    DeleteRel {
        id: RelId,
    },
    CreateIndex {
        label: String,
        key: String,
    },
    DropIndex {
        label: String,
        key: String,
    },
    CreateConstraint {
        kind: ConstraintKind,
        label: String,
        key: String,
    },
    DropConstraint {
        kind: ConstraintKind,
        label: String,
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteBatch {
    pub epoch: u64,
    pub commands: Vec<Command>,
}

fn put_kind(out: &mut Vec<u8>, kind: ConstraintKind) {
    out.push(match kind {
        ConstraintKind::Unique => 0,
        ConstraintKind::Exists => 1,
    });
}

fn invalid(msg: &str) -> Error {
    Error::InvalidBatch(msg.to_string())
}

impl Command {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Command::CreateNode { id, labels, props } => {
                out.push(1);
                out.extend_from_slice(&id.0.to_le_bytes());
                put_u32(out, labels.len());
                for l in labels {
                    put_str(out, l);
                }
                put_props(out, props);
            }
            Command::SetNodeProps { id, props } => {
                out.push(2);
                out.extend_from_slice(&id.0.to_le_bytes());
                put_props(out, props);
            }
            Command::AddLabel { id, label } => {
                out.push(3);
                out.extend_from_slice(&id.0.to_le_bytes());
                put_str(out, label);
            }
            Command::RemoveLabel { id, label } => {
                out.push(4);
                out.extend_from_slice(&id.0.to_le_bytes());
                put_str(out, label);
            }
            Command::DeleteNode { id } => {
                out.push(5);
                out.extend_from_slice(&id.0.to_le_bytes());
            }
            Command::CreateRel {
                id,
                src,
                dst,
                rel_type,
                props,
            } => {
                out.push(6);
                out.extend_from_slice(&id.0.to_le_bytes());
                out.extend_from_slice(&src.0.to_le_bytes());
                out.extend_from_slice(&dst.0.to_le_bytes());
                put_str(out, rel_type);
                put_props(out, props);
            }
            Command::SetRelProps { id, props } => {
                out.push(7);
                out.extend_from_slice(&id.0.to_le_bytes());
                put_props(out, props);
            }
            Command::DeleteRel { id } => {
                out.push(8);
                out.extend_from_slice(&id.0.to_le_bytes());
            }
            Command::CreateIndex { label, key } => {
                out.push(9);
                put_str(out, label);
                put_str(out, key);
            }
            Command::DropIndex { label, key } => {
                out.push(10);
                put_str(out, label);
                put_str(out, key);
            }
            Command::CreateConstraint { kind, label, key } => {
                out.push(11);
                put_kind(out, *kind);
                put_str(out, label);
                put_str(out, key);
            }
            Command::DropConstraint { kind, label, key } => {
                out.push(12);
                put_kind(out, *kind);
                put_str(out, label);
                put_str(out, key);
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Command> {
        fn kind(r: &mut Reader<'_>) -> Result<ConstraintKind> {
            match r.u8()? {
                0 => Ok(ConstraintKind::Unique),
                1 => Ok(ConstraintKind::Exists),
                other => Err(Error::Corrupt(format!("unknown constraint kind {other}"))),
            }
        }
        Ok(match r.u8()? {
            1 => {
                let id = NodeId(r.u64()?);
                let n = r.count(4)?;
                let mut labels = BTreeSet::new();
                for _ in 0..n {
                    labels.insert(r.string()?);
                }
                Command::CreateNode {
                    id,
                    labels,
                    props: r.props()?,
                }
            }
            2 => Command::SetNodeProps {
                id: NodeId(r.u64()?),
                props: r.props()?,
            },
            3 => Command::AddLabel {
                id: NodeId(r.u64()?),
                label: r.string()?,
            },
            4 => Command::RemoveLabel {
                id: NodeId(r.u64()?),
                label: r.string()?,
            },
            5 => Command::DeleteNode {
                id: NodeId(r.u64()?),
            },
            6 => Command::CreateRel {
                id: RelId(r.u64()?),
                src: NodeId(r.u64()?),
                dst: NodeId(r.u64()?),
                rel_type: r.string()?,
                props: r.props()?,
            },
            7 => Command::SetRelProps {
                id: RelId(r.u64()?),
                props: r.props()?,
            },
            8 => Command::DeleteRel {
                id: RelId(r.u64()?),
            },
            9 => Command::CreateIndex {
                label: r.string()?,
                key: r.string()?,
            },
            10 => Command::DropIndex {
                label: r.string()?,
                key: r.string()?,
            },
            11 => Command::CreateConstraint {
                kind: kind(r)?,
                label: r.string()?,
                key: r.string()?,
            },
            12 => Command::DropConstraint {
                kind: kind(r)?,
                label: r.string()?,
                key: r.string()?,
            },
            other => return Err(Error::Corrupt(format!("unknown command tag {other}"))),
        })
    }
}

impl WriteBatch {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![BATCH_VERSION];
        out.extend_from_slice(&self.epoch.to_le_bytes());
        put_u32(&mut out, self.commands.len());
        for c in &self.commands {
            c.encode(&mut out);
        }
        let crc = crc32c(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<WriteBatch> {
        if bytes.len() < HEADER_BYTES + CRC_BYTES {
            return Err(invalid("batch truncated"));
        }
        let (body, tail) = bytes.split_at(bytes.len() - CRC_BYTES);
        let stored = u32::from_le_bytes(tail.try_into().expect("4 bytes"));
        if stored != crc32c(body) {
            return Err(invalid("batch checksum mismatch"));
        }
        Self::decode_body(body).map_err(|e| match e {
            Error::Corrupt(m) => Error::InvalidBatch(m),
            other => other,
        })
    }

    fn decode_body(body: &[u8]) -> Result<WriteBatch> {
        let mut r = Reader::new(body);
        let version = r.u8()?;
        if version != BATCH_VERSION {
            return Err(Error::InvalidBatch(format!(
                "unknown batch version {version}"
            )));
        }
        let epoch = r.u64()?;
        let n = r.count(MIN_COMMAND_BYTES)?;
        let mut commands = Vec::with_capacity(n);
        for _ in 0..n {
            commands.push(Command::decode(&mut r)?);
        }
        r.finish()?;
        Ok(WriteBatch { epoch, commands })
    }
}

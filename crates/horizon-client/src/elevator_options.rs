use std::sync::atomic::{AtomicU64, Ordering};
pub const CHANNEL: &str = "horizon:elevator_options";
/// Native editor policy: vanilla block reach plus the same four-block buffer
/// used by Horizon's container editors. The original mod's exact radius is unverified.
pub fn within_editor_reach(eye: [f64; 3], creative: bool, pos: [i32; 3]) -> bool {
    let reach = if creative { 5.0 } else { 4.5 };
    crate::lan::guest::within_block_range(eye, reach + 4.0, pos)
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub fn identity() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Heading {
    #[default]
    North,
    East,
    South,
    West,
}
impl Heading {
    pub fn index(self) -> u8 {
        match self {
            Self::North => 0,
            Self::East => 1,
            Self::South => 2,
            Self::West => 3,
        }
    }
    pub fn from_index(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::North,
            1 => Self::East,
            2 => Self::South,
            3 => Self::West,
            _ => return None,
        })
    }
    pub fn yaw(self) -> f32 {
        match self {
            Self::North => 180.0,
            Self::East => 270.0,
            Self::South => 0.0,
            Self::West => 90.0,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ElevatorOptions {
    pub directional: bool,
    pub heading: Heading,
    pub hide_arrow: bool,
}
impl ElevatorOptions {
    pub fn bits(self) -> u8 {
        self.heading.index() | (u8::from(self.directional) << 2) | (u8::from(self.hide_arrow) << 3)
    }
    pub fn from_bits(bits: u8) -> Option<Self> {
        (bits < 16).then(|| Self {
            directional: bits & 4 != 0,
            heading: Heading::from_index(bits & 3).unwrap(),
            hide_arrow: bits & 8 != 0,
        })
    }
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Directional(v) => self.directional = v,
            Action::Heading(v) => self.heading = v,
            Action::HideArrow(v) => self.hide_arrow = v,
            Action::Close => {}
        }
    }
}
#[derive(Clone, Debug)]
pub struct ElevatorBlock {
    pub options: ElevatorOptions,
    pub instance_id: u64,
}
impl ElevatorBlock {
    pub fn renew_instance(&mut self) {
        self.instance_id = identity();
    }
}
impl Default for ElevatorBlock {
    fn default() -> Self {
        Self {
            options: ElevatorOptions::default(),
            instance_id: identity(),
        }
    }
}
pub fn is_elevator(name: &str) -> bool {
    name.strip_prefix("elevatorid:elevator_")
        .is_some_and(|color| {
            matches!(
                color,
                "white"
                    | "orange"
                    | "magenta"
                    | "light_blue"
                    | "yellow"
                    | "lime"
                    | "pink"
                    | "gray"
                    | "light_gray"
                    | "cyan"
                    | "purple"
                    | "blue"
                    | "brown"
                    | "green"
                    | "red"
                    | "black"
            )
        })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Directional(bool),
    Heading(Heading),
    HideArrow(bool),
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Open {
        session: u64,
        pos: [i32; 3],
        state: ElevatorOptions,
    },
    Action {
        session: u64,
        action: Action,
    },
    State {
        pos: [i32; 3],
        state: ElevatorOptions,
    },
    Chunk {
        x: i32,
        z: i32,
        entries: Vec<([i32; 3], ElevatorOptions)>,
    },
    Close {
        session: u64,
    },
}
impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![3];
        match self {
            Self::Open {
                session,
                pos,
                state,
            } => {
                out.push(0);
                out.extend(session.to_be_bytes());
                for v in pos {
                    out.extend(v.to_be_bytes());
                }
                out.push(state.bits());
            }
            Self::Action { session, action } => {
                out.push(1);
                out.extend(session.to_be_bytes());
                let (tag, value) = match action {
                    Action::Directional(v) => (0, u8::from(*v)),
                    Action::Heading(v) => (1, v.index()),
                    Action::HideArrow(v) => (2, u8::from(*v)),
                    Action::Close => (3, 0),
                };
                out.extend([tag, value]);
            }
            Self::State { pos, state } => {
                out.push(2);
                for v in pos {
                    out.extend(v.to_be_bytes());
                }
                out.push(state.bits());
            }
            Self::Chunk { x, z, entries } => {
                out.push(3);
                out.extend(x.to_be_bytes());
                out.extend(z.to_be_bytes());
                out.extend((entries.len() as u32).to_be_bytes());
                for (p, o) in entries {
                    let packed = ((p[0] & 15) as u32)
                        | (((p[2] & 15) as u32) << 4)
                        | (((p[1] - crate::chunk::MIN_Y) as u32) << 8);
                    out.extend(packed.to_be_bytes());
                    out.push(o.bits());
                }
            }
            Self::Close { session } => {
                out.push(4);
                out.extend(session.to_be_bytes());
            }
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = Read(bytes);
        if bytes.len() > 512 * 1024 || r.byte()? != 3 {
            return None;
        }
        let result = match r.byte()? {
            0 => Self::Open {
                session: r.u64()?,
                pos: r.pos()?,
                state: ElevatorOptions::from_bits(r.byte()?)?,
            },
            1 => {
                let session = r.u64()?;
                let tag = r.byte()?;
                let value = r.byte()?;
                let action = match (tag, value) {
                    (0, 0..=1) => Action::Directional(value != 0),
                    (1, 0..=3) => Action::Heading(Heading::from_index(value)?),
                    (2, 0..=1) => Action::HideArrow(value != 0),
                    (3, 0) => Action::Close,
                    _ => return None,
                };
                Self::Action { session, action }
            }
            2 => Self::State {
                pos: r.pos()?,
                state: ElevatorOptions::from_bits(r.byte()?)?,
            },
            3 => {
                let x = r.i32()?;
                let z = r.i32()?;
                if x.unsigned_abs() > 1_875_000 || z.unsigned_abs() > 1_875_000 {
                    return None;
                }
                let count = r.u32()? as usize;
                if count > crate::chunk::HEIGHT as usize * 256
                    || r.0.len() < count.checked_mul(5)?
                {
                    return None;
                }
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let p = r.u32()?;
                    if p >> 17 != 0 || ((p >> 8) & 511) >= crate::chunk::HEIGHT as u32 {
                        return None;
                    }
                    entries.push((
                        [
                            x * 16 + (p & 15) as i32,
                            crate::chunk::MIN_Y + ((p >> 8) & 511) as i32,
                            z * 16 + ((p >> 4) & 15) as i32,
                        ],
                        ElevatorOptions::from_bits(r.byte()?)?,
                    ));
                }
                Self::Chunk { x, z, entries }
            }
            4 => Self::Close { session: r.u64()? },
            _ => return None,
        };
        r.0.is_empty().then_some(result)
    }
    pub fn packet(&self) -> horizon_protocol::packets::configuration::CustomPayload {
        horizon_protocol::packets::configuration::CustomPayload {
            channel: CHANNEL.into(),
            data: self.encode(),
        }
    }
}
struct Read<'a>(&'a [u8]);
impl Read<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, tail) = self.0.split_at_checked(N)?;
        self.0 = tail;
        head.try_into().ok()
    }
    fn byte(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take()?))
    }
    fn pos(&mut self) -> Option<[i32; 3]> {
        let p = [self.i32()?, self.i32()?, self.i32()?];
        (p[0].unsigned_abs() <= 30_000_000
            && p[2].unsigned_abs() <= 30_000_000
            && (crate::chunk::MIN_Y..crate::chunk::MIN_Y + crate::chunk::HEIGHT).contains(&p[1]))
        .then_some(p)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elevator_protocol_roundtrip_and_bounds() {
        let options = ElevatorOptions {
            directional: true,
            heading: Heading::East,
            hide_arrow: true,
        };
        let messages = [
            Message::Open {
                session: 9,
                pos: [-2, 170, 4],
                state: options,
            },
            Message::Action {
                session: 9,
                action: Action::Heading(Heading::West),
            },
            Message::Chunk {
                x: -1,
                z: 0,
                entries: vec![([-2, 170, 4], options)],
            },
        ];
        for msg in messages {
            let bytes = msg.encode();
            assert_eq!(Message::decode(&bytes), Some(msg));
            assert!(Message::decode(&bytes[..bytes.len() - 1]).is_none());
            let mut bad = bytes;
            bad.push(0);
            assert!(Message::decode(&bad).is_none());
        }
        assert!(Message::decode(&[1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255]).is_none());
        for value in [4, 255] {
            let mut bytes = Message::Action {
                session: 1,
                action: Action::Heading(Heading::North),
            }
            .encode();
            *bytes.last_mut().unwrap() = value;
            assert!(Message::decode(&bytes).is_none());
        }
    }
    #[test]
    fn elevator_full_column_snapshot_fits_frame() {
        let entries: Vec<_> = (0..crate::chunk::HEIGHT * 256)
            .map(|i| {
                (
                    [i & 15, crate::chunk::MIN_Y + (i >> 8), (i >> 4) & 15],
                    ElevatorOptions::default(),
                )
            })
            .collect();
        let message = Message::Chunk {
            x: 0,
            z: 0,
            entries,
        };
        let bytes = message.encode();
        assert!(bytes.len() < 512 * 1024);
        assert_eq!(Message::decode(&bytes), Some(message));
    }
    #[test]
    fn elevator_settings_persist_but_instance_identity_does_not() {
        use crate::{
            containers::BlockEntity,
            save::{decode_block_entity, encode_block_entity},
        };
        use horizon_world::nbt::Tag;
        let original = ElevatorBlock {
            options: ElevatorOptions {
                directional: true,
                heading: Heading::East,
                hide_arrow: true,
            },
            ..Default::default()
        };
        let mut snapshot = original.clone();
        assert_eq!(snapshot.instance_id, original.instance_id);
        snapshot.renew_instance();
        assert_ne!(snapshot.instance_id, original.instance_id);
        assert_eq!(snapshot.options, original.options);
        let tag = encode_block_entity(
            (1, 170, 2),
            &BlockEntity::Elevator(original.clone()),
            "elevatorid:elevator_white",
        );
        let (_, BlockEntity::Elevator(loaded)) = decode_block_entity(&tag).unwrap() else {
            panic!("wrong type")
        };
        assert_eq!(loaded.options, original.options);
        assert_ne!(loaded.instance_id, original.instance_id);
        let mut legacy = tag.clone();
        if let Tag::Compound(c) = &mut legacy {
            c.insert("Camouflage".into(), Tag::String("minecraft:stone".into()));
        }
        let (_, BlockEntity::Elevator(loaded)) = decode_block_entity(&legacy).unwrap() else {
            panic!("wrong type")
        };
        assert_eq!(loaded.options, original.options);
        assert!(
            encode_block_entity(
                (1, 170, 2),
                &BlockEntity::Elevator(loaded),
                "elevatorid:elevator_white"
            )
            .get("Camouflage")
            .is_none()
        );
        for value in [-256, 256, 16] {
            let mut bad = tag.clone();
            if let Tag::Compound(c) = &mut bad {
                c.insert("Options".into(), Tag::Int(value));
            }
            assert!(decode_block_entity(&bad).is_none());
        }
    }
    #[test]
    fn elevator_replacement_discards_old_settings() {
        use crate::{chunk::Chunk, containers::BlockEntity, world::World};
        use horizon_world::registry::BlockStateId;
        use std::sync::Arc;
        let mut world = World::new(BlockStateId(0), 0);
        world.chunks.insert((0, 0), Arc::new(Chunk::empty(0, 0)));
        world.set(1, 0, 1, BlockStateId(1)).unwrap();
        world
            .block_entities
            .insert((1, 0, 1), BlockEntity::Elevator(Default::default()));
        world.set(1, 0, 1, BlockStateId(0)).unwrap();
        assert!(world.block_entities.get(&(1, 0, 1)).is_none());
    }
    #[test]
    fn elevator_toggle_retains_heading_and_arrow() {
        let mut o = ElevatorOptions::default();
        o.apply(Action::Heading(Heading::East));
        o.apply(Action::HideArrow(true));
        o.apply(Action::Directional(true));
        o.apply(Action::Directional(false));
        assert_eq!(
            o,
            ElevatorOptions {
                directional: false,
                heading: Heading::East,
                hide_arrow: true
            }
        );
        assert_eq!(o.heading.yaw(), 270.0);
    }
}

#[cfg(test)]
mod reach_tests {
    #[test]
    fn elevator_editor_reach_uses_eye_to_box_with_mode_buffer() {
        let pos = [0, 0, 0];
        for (creative, radius) in [(false, 8.5), (true, 9.0)] {
            assert!(super::within_editor_reach(
                [1.0 + radius - 0.001, 0.5, 0.5],
                creative,
                pos
            ));
            assert!(!super::within_editor_reach(
                [1.0 + radius, 0.5, 0.5],
                creative,
                pos
            ));
            assert!(!super::within_editor_reach(
                [-radius, 0.5, 0.5],
                creative,
                pos
            ));
            assert!(super::within_editor_reach(
                [0.5, 1.0 + radius - 0.001, 0.5],
                creative,
                pos
            ));
        }
    }
}

//! Original native core shared by the two optional compass items.
use crate::items::ItemStack;
use horizon_world::nbt::Tag;
use serde::{Deserialize, Serialize};
pub const CHANNEL: &str = "horizon:compass";
/// A persisted stack nonce; unlike UI sessions this must survive process restarts.
/// RandomState uses OS-seeded keys, as the local control token generator does.
pub fn stack_identity() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut hash = std::collections::hash_map::RandomState::new().build_hasher();
    hash.write_u64(NEXT.fetch_add(1, Ordering::Relaxed));
    hash.write_u32(std::process::id());
    hash.finish().max(1)
}

const DATA: &str = "minecraft:custom_data";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompassKind {
    Biome,
    Structure,
}
impl CompassKind {
    pub fn of(item: &str) -> Option<Self> {
        match item {
            "naturescompass:naturescompass" => Some(Self::Biome),
            "explorerscompass:explorerscompass" => Some(Self::Structure),
            _ => None,
        }
    }
    pub fn mod_id(self) -> &'static str {
        match self {
            Self::Biome => "naturescompass",
            Self::Structure => "explorerscompass",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub dimension: String,
    pub pos: [i32; 3],
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub identity: u64,
    pub target: Option<Target>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    Open,
    Search {
        session: u64,
        target: String,
    },
    Reset {
        session: u64,
    },
    Close {
        session: u64,
    },
    View {
        open: bool,
        slot: usize,
        kind: CompassKind,
        dimension: String,
        identity: u64,
        session: u64,
        catalog: Vec<String>,
        status: String,
        target: Option<Target>,
    },
}
impl Message {
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() > 131072 {
            return None;
        }
        serde_json::from_slice(data).ok()
    }
    pub fn packet(&self) -> horizon_protocol::packets::configuration::CustomPayload {
        horizon_protocol::packets::configuration::CustomPayload {
            channel: CHANNEL.into(),
            data: serde_json::to_vec(self).expect("compass message"),
        }
    }
}
pub fn state(stack: &ItemStack) -> State {
    stack
        .data()
        .and_then(|d| d.other(DATA))
        .and_then(|t| t.get("horizon_compass"))
        .and_then(|t| {
            if let Tag::String(s) = t {
                s.to_string().ok()
            } else {
                None
            }
        })
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
pub fn write(stack: &mut ItemStack, state: &State) {
    let mut data = stack
        .data()
        .and_then(|d| d.other(DATA))
        .cloned()
        .unwrap_or_else(|| Tag::Compound(Default::default()));
    if let Tag::Compound(entries) = &mut data {
        entries.insert(
            "horizon_compass".into(),
            Tag::String(
                serde_json::to_string(state)
                    .expect("compass state")
                    .as_str()
                    .into(),
            ),
        );
    }
    stack.data_mut().set_other(DATA, Some(data));
    refresh_needle(stack);
}
pub fn refresh_needle(stack: &mut ItemStack) {
    if CompassKind::of(&stack.item).is_none() {
        return;
    }
    let target = state(stack).target.map(|t| {
        Tag::compound([
            ("tracked", Tag::Byte(0)),
            (
                "target",
                Tag::compound([
                    ("dimension", Tag::String(t.dimension.as_str().into())),
                    ("pos", Tag::IntArray(t.pos.to_vec())),
                ]),
            ),
        ])
    });
    stack
        .data_mut()
        .set_other("minecraft:lodestone_tracker", target);
}
pub fn navigation(target: &Target, dimension: &str, pos: [f64; 3]) -> String {
    if target.dimension != dimension {
        return "Wrong dimension".into();
    }
    let dx = f64::from(target.pos[0]) + 0.5 - pos[0];
    let dz = f64::from(target.pos[2]) + 0.5 - pos[2];
    let direction = ["S", "SW", "W", "NW", "N", "NE", "E", "SE"]
        [(((-dx).atan2(dz).to_degrees() / 45.0).round() as i32).rem_euclid(8) as usize];
    format!(
        "{} | {} | {:.0} blocks",
        target.name,
        direction,
        dx.hypot(dz)
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_roundtrip_reset_preserves_other_custom_data() {
        let mut stack = ItemStack::new("naturescompass:naturescompass", 1);
        stack
            .data_mut()
            .set_other(DATA, Some(Tag::compound([("other", Tag::Int(7))])));
        let mut s = State {
            identity: 9,
            target: Some(Target {
                name: "minecraft:plains".into(),
                dimension: "minecraft:overworld".into(),
                pos: [12, 64, -3],
            }),
        };
        write(&mut stack, &s);
        let (damage, extra) = crate::item_data::decode_components(
            crate::item_data::encode_components(&stack).as_ref(),
        );
        let mut restored = ItemStack::new("naturescompass:naturescompass", 1);
        restored.damage = damage;
        restored.extra = extra;
        assert_eq!(state(&restored), s);
        assert!(
            restored
                .data()
                .unwrap()
                .other("minecraft:lodestone_tracker")
                .is_some()
        );
        s.target = None;
        write(&mut restored, &s);
        assert!(
            restored
                .data()
                .unwrap()
                .other("minecraft:lodestone_tracker")
                .is_none()
        );
        assert_eq!(
            restored.data().unwrap().other(DATA).unwrap().get("other"),
            Some(&Tag::Int(7))
        );
    }
    #[test]
    fn message_and_navigation() {
        let p = Message::Search {
            session: 42,
            target: "minecraft:plains".into(),
        }
        .packet();
        assert!(matches!(
            Message::decode(&p.data),
            Some(Message::Search { session: 42, .. })
        ));
        assert!(Message::decode(&vec![0; 131073]).is_none());
        let t = Target {
            name: "plains".into(),
            dimension: "overworld".into(),
            pos: [100, 64, 0],
        };
        assert!(navigation(&t, "overworld", [0.5, 64., 0.5]).contains("E | 100"));
        assert_eq!(navigation(&t, "nether", [0.; 3]), "Wrong dimension");
    }
}

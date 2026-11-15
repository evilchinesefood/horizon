//! Bounded, deterministic planning for the ore-only excavation pilot.
use std::collections::{HashSet, VecDeque};
pub const CHANNEL: &str = "horizon:ore_excavation";
pub const KEY: &str = "key.oreexcavation.excavate";
pub const MAX_BLOCKS: usize = 64;
pub const RADIUS: i32 = 8;
pub const BREAKS_PER_TICK: usize = 4;
pub type Pos = [i32; 3];
/// Ordinary mining and Mending both change damage without replacing the tool.
pub fn same_tool(a: &crate::items::ItemStack, b: &crate::items::ItemStack) -> bool {
    a.item == b.item && a.count == b.count && a.extra == b.extra
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OreFamily {
    Coal,
    Iron,
    Copper,
    Gold,
    Redstone,
    Emerald,
    Lapis,
    Diamond,
    Quartz,
    AncientDebris,
}
pub fn ore_family(name: &str) -> Option<OreFamily> {
    use OreFamily::*;
    Some(match name {
        "minecraft:coal_ore" | "minecraft:deepslate_coal_ore" => Coal,
        "minecraft:iron_ore" | "minecraft:deepslate_iron_ore" => Iron,
        "minecraft:copper_ore" | "minecraft:deepslate_copper_ore" => Copper,
        "minecraft:gold_ore" | "minecraft:deepslate_gold_ore" | "minecraft:nether_gold_ore" => Gold,
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore" => Redstone,
        "minecraft:emerald_ore" | "minecraft:deepslate_emerald_ore" => Emerald,
        "minecraft:lapis_ore" | "minecraft:deepslate_lapis_ore" => Lapis,
        "minecraft:diamond_ore" | "minecraft:deepslate_diamond_ore" => Diamond,
        "minecraft:nether_quartz_ore" => Quartz,
        "minecraft:ancient_debris" => AncientDebris,
        _ => return None,
    })
}
pub fn held_intent(data: &[u8]) -> Option<bool> {
    match data {
        [0] => Some(false),
        [1] => Some(true),
        _ => None,
    }
}
#[derive(Clone, Debug)]
struct Node {
    pos: Pos,
    parent: usize,
}
#[derive(Clone, Debug)]
pub struct Plan {
    nodes: Vec<Node>,
    next: usize,
    removed: Vec<bool>,
}
impl Plan {
    /// The seed has already been successfully removed by ordinary mining.
    /// `read` returns None for absent chunks and blocks outside this ore family.
    pub fn build(
        seed: Pos,
        family: OreFamily,
        mut read: impl FnMut(Pos) -> Option<OreFamily>,
    ) -> Self {
        let mut nodes = vec![Node {
            pos: seed,
            parent: 0,
        }];
        let mut seen = HashSet::from([seed]);
        let mut frontier = VecDeque::from([0]);
        while let Some(parent) = frontier.pop_front() {
            for (axis, delta) in [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
                if nodes.len() == MAX_BLOCKS {
                    break;
                }
                let mut pos = nodes[parent].pos;
                let Some(value) = pos[axis].checked_add(delta) else {
                    continue;
                };
                pos[axis] = value;
                if pos
                    .iter()
                    .zip(seed)
                    .any(|(&v, s)| i64::from(v).abs_diff(i64::from(s)) > RADIUS as u64)
                    || !seen.insert(pos)
                    || read(pos) != Some(family)
                {
                    continue;
                }
                frontier.push_back(nodes.len());
                nodes.push(Node { pos, parent });
            }
        }
        let mut removed = vec![false; nodes.len()];
        removed[0] = true;
        Self {
            nodes,
            next: 1,
            removed,
        }
    }
    pub fn remaining(&self) -> usize {
        self.nodes.len() - self.next
    }
    /// Failed/replaced parents prune their entire branch. Successful parents
    /// are recorded here, since the world now contains air at those positions.
    pub fn next_candidate(&mut self) -> Option<(usize, Pos)> {
        while self.next < self.nodes.len() {
            let index = self.next;
            self.next += 1;
            let node = &self.nodes[index];
            if self.removed[node.parent] {
                return Some((index, node.pos));
            }
        }
        None
    }
    pub fn confirm_removed(&mut self, index: usize) {
        self.removed[index] = true;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_guard_accepts_wear_and_mending_but_rejects_replacement() {
        let mut original = crate::items::ItemStack::new("minecraft:diamond_pickaxe", 1);
        original.damage = 10;
        let mut current = original.clone();
        current.damage = 11;
        assert!(same_tool(&original, &current));
        current.damage = 5;
        assert!(same_tool(&original, &current));
        current.item = "minecraft:iron_pickaxe".into();
        assert!(!same_tool(&original, &current));
        current = original.clone();
        current.data_mut().set_other(
            "minecraft:custom_data",
            Some(horizon_world::nbt::Tag::compound([(
                "changed",
                horizon_world::nbt::Tag::Int(1),
            )])),
        );
        assert!(!same_tool(&original, &current));
        current = original.clone();
        current.count = 2;
        assert!(!same_tool(&original, &current));
    }
    #[test]
    fn explicit_families_and_intent() {
        assert_eq!(
            ore_family("minecraft:deepslate_diamond_ore"),
            ore_family("minecraft:diamond_ore")
        );
        assert_eq!(ore_family("minecraft:oak_log"), None);
        assert_eq!(held_intent(&[1]), Some(true));
        assert_eq!(held_intent(&[1, 0]), None);
    }
    #[test]
    fn cap_and_radius_are_bounded() {
        let plan = Plan::build([0; 3], OreFamily::Iron, |_| Some(OreFamily::Iron));
        assert_eq!(plan.nodes.len(), MAX_BLOCKS);
        let line = Plan::build([0; 3], OreFamily::Iron, |p| {
            (p[1] == 0 && p[2] == 0).then_some(OreFamily::Iron)
        });
        assert_eq!(line.nodes.len(), 17);
    }
    #[test]
    fn diagonal_unloaded_and_other_ores_do_not_connect() {
        let plan = Plan::build([0; 3], OreFamily::Iron, |p| match p {
            [1, 1, 0] => Some(OreFamily::Iron),
            [1, 0, 0] => Some(OreFamily::Gold),
            _ => None,
        });
        assert_eq!(plan.remaining(), 0);
    }
    #[test]
    fn failed_parent_cuts_branch_successful_air_parent_does_not() {
        let line = || {
            Plan::build([0; 3], OreFamily::Iron, |p| {
                (p[0] > 0 && p[0] < 4 && p[1] == 0 && p[2] == 0).then_some(OreFamily::Iron)
            })
        };
        let mut failed = line();
        assert_eq!(failed.next_candidate().unwrap().1, [1, 0, 0]);
        assert_eq!(failed.next_candidate(), None);
        let mut success = line();
        let (i, _) = success.next_candidate().unwrap();
        success.confirm_removed(i);
        assert_eq!(success.next_candidate().unwrap().1, [2, 0, 0]);
    }
}

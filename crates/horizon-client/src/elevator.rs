//! Opt-in vertical travel for declared elevator blocks.
use serde::Deserialize;
use std::{collections::HashSet, io::ErrorKind, path::Path};
pub const DECLARATION: &str = "assets/elevatorid/horizon/elevators.json";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}
pub fn pressed(previous: u8, current: u8) -> Option<Direction> {
    match (current & !previous) & 48 {
        16 if current & 32 == 0 => Some(Direction::Up),
        32 if current & 16 == 0 => Some(Direction::Down),
        _ => None,
    }
}
#[derive(Clone, Debug)]
pub struct Activation {
    pub direction: Direction,
    pub position: [f64; 3],
    pub grounded: bool,
    pub dimension: String,
    pub teleport: i32,
    pub tick: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rules {
    enabled: bool,
    blocks: Vec<String>,
    range: i32,
    same_color: bool,
    precision_target: bool,
}
#[derive(Default, Debug)]
pub struct Elevators(Option<Rules>);
pub trait World {
    fn block(&self, position: [i32; 3]) -> Option<&str>;
    fn can_land(&mut self, position: [f64; 3]) -> crate::Result<bool>;
}
impl Elevators {
    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }
    pub fn load(root: &Path) -> crate::Result<Self> {
        match std::fs::read(root.join(DECLARATION)) {
            Ok(bytes) => Self::parse(&bytes),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn parse(bytes: &[u8]) -> crate::Result<Self> {
        let rules: Rules = serde_json::from_slice(bytes)?;
        if !(3..=4064).contains(&rules.range)
            || rules.blocks.is_empty()
            || rules.blocks.len() > 256
            || rules
                .blocks
                .iter()
                .any(|b| !b.starts_with("elevatorid:elevator_"))
            || rules.blocks.iter().collect::<HashSet<_>>().len() != rules.blocks.len()
        {
            return Err("invalid elevator block declaration or range".into());
        }
        Ok(Self(rules.enabled.then_some(rules)))
    }
    pub fn validate(&self, registry: &horizon_world::registry::BlockRegistry) -> crate::Result<()> {
        if let Some(rules) = &self.0 {
            for block in &rules.blocks {
                if registry.default_state(block).is_none() {
                    return Err(format!("elevator block is not registered: {block}").into());
                }
            }
        }
        Ok(())
    }
    pub fn destination(
        &self,
        world: &mut impl World,
        feet: [f64; 3],
        direction: Direction,
    ) -> crate::Result<Option<[f64; 3]>> {
        let Some(rules) = &self.0 else {
            return Ok(None);
        };
        if feet.iter().any(|v| !v.is_finite()) || (feet[1] - feet[1].round()).abs() > 0.001 {
            return Ok(None);
        }
        let Some(source_y) = (feet[1].round() as i32).checked_sub(1) else {
            return Ok(None);
        };
        let origin = [feet[0].floor() as i32, source_y, feet[2].floor() as i32];
        let Some(color) = world
            .block(origin)
            .and_then(|b| rules.blocks.iter().position(|id| id == b))
        else {
            return Ok(None);
        };
        let step = if direction == Direction::Up { 1 } else { -1 };
        for distance in 1..=rules.range {
            let Some(y) = origin[1].checked_add(distance * step) else {
                break;
            };
            let candidate = world
                .block([origin[0], y, origin[2]])
                .and_then(|b| rules.blocks.iter().position(|id| id == b));
            if candidate.is_none() || (rules.same_color && candidate != Some(color)) {
                continue;
            }
            let at = if !rules.precision_target {
                [feet[0], f64::from(y) + 1.0, feet[2]]
            } else {
                [
                    f64::from(origin[0]) + 0.5,
                    f64::from(y) + 1.0,
                    f64::from(origin[2]) + 0.5,
                ]
            };
            if world.can_land(at)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
}
pub fn loaded_landing(
    bounds: horizon_core::geometry::Aabb,
    mut loaded: impl FnMut(i32, i32) -> bool,
) -> bool {
    use crate::chunk::{HEIGHT, MIN_Y};
    use horizon_core::geometry::Axis;
    if bounds.min(Axis::Y) < f64::from(MIN_Y) || bounds.max(Axis::Y) > f64::from(MIN_Y + HEIGHT) {
        return false;
    }
    for x in
        (bounds.min(Axis::X).floor() as i32 >> 4)..=((bounds.max(Axis::X).ceil() as i32 - 1) >> 4)
    {
        for z in (bounds.min(Axis::Z).floor() as i32 >> 4)
            ..=((bounds.max(Axis::Z).ceil() as i32 - 1) >> 4)
        {
            if !loaded(x, z) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scene {
        blocks: Vec<([i32; 3], &'static str)>,
        blocked: Vec<i32>,
    }
    impl World for Scene {
        fn block(&self, p: [i32; 3]) -> Option<&str> {
            self.blocks.iter().find(|(at, _)| *at == p).map(|(_, b)| *b)
        }
        fn can_land(&mut self, p: [f64; 3]) -> crate::Result<bool> {
            Ok(!self.blocked.contains(&(p[1] as i32)))
        }
    }
    fn rules(same: bool, precision: bool) -> Elevators {
        Elevators::parse(format!(r#"{{"enabled":true,"blocks":["elevatorid:elevator_white","elevatorid:elevator_red"],"range":8,"same_color":{same},"precision_target":{precision}}}"#).as_bytes()).unwrap()
    }
    fn scene() -> Scene {
        Scene {
            blocks: vec![
                ([0, 0, 0], "elevatorid:elevator_white"),
                ([0, 4, 0], "elevatorid:elevator_red"),
                ([0, 8, 0], "elevatorid:elevator_white"),
            ],
            blocked: vec![],
        }
    }
    #[test]
    fn landing_requires_all_overlapped_chunks_and_build_height() {
        use horizon_core::geometry::{Aabb, Vec3};
        let crossing = Aabb::new(Vec3::new(15.5, 10.0, 15.5), Vec3::new(16.1, 11.8, 16.1));
        assert!(!loaded_landing(crossing, |x, z| (x, z) != (1, 1)));
        assert!(loaded_landing(crossing, |_, _| true));
        let boundary = Aabb::new(Vec3::new(15.4, 10.0, 15.4), Vec3::new(16.0, 11.8, 16.0));
        assert!(loaded_landing(boundary, |x, z| (x, z) == (0, 0)));
        let ceiling = Aabb::new(Vec3::new(0.2, 319.0, 0.2), Vec3::new(0.8, 320.8, 0.8));
        assert!(!loaded_landing(ceiling, |_, _| true));
    }
    #[test]
    fn nearest_clear_landing_in_both_directions_preserves_precision() {
        let mut world = scene();
        let r = rules(false, false);
        assert_eq!(
            r.destination(&mut world, [0.3, 1.0, 0.7], Direction::Up)
                .unwrap(),
            Some([0.3, 5.0, 0.7])
        );
        assert_eq!(
            r.destination(&mut world, [0.3, 9.0, 0.7], Direction::Down)
                .unwrap(),
            Some([0.3, 5.0, 0.7])
        );
        world.blocked.push(5);
        assert_eq!(
            r.destination(&mut world, [0.3, 1.0, 0.7], Direction::Up)
                .unwrap(),
            Some([0.3, 9.0, 0.7])
        );
    }
    #[test]
    fn color_range_alignment_and_centering_are_enforced() {
        let mut world = scene();
        let r = rules(true, true);
        assert_eq!(
            r.destination(&mut world, [0.2, 1.0, 0.8], Direction::Up)
                .unwrap(),
            Some([0.5, 9.0, 0.5])
        );
        world.blocks[2].0[1] = 9;
        assert_eq!(
            r.destination(&mut world, [0.2, 1.0, 0.8], Direction::Up)
                .unwrap(),
            None
        );
        world.blocks[2].0 = [1, 8, 0];
        assert_eq!(
            r.destination(&mut world, [0.2, 1.0, 0.8], Direction::Up)
                .unwrap(),
            None
        );
        assert_eq!(
            r.destination(&mut world, [0.2, 1.4, 0.8], Direction::Up)
                .unwrap(),
            None
        );
    }
    #[test]
    fn activation_requires_new_unambiguous_press() {
        assert_eq!(pressed(0, 16), Some(Direction::Up));
        assert_eq!(pressed(16, 16), None);
        assert_eq!(pressed(16, 0), None);
        assert_eq!(pressed(0, 32), Some(Direction::Down));
        assert_eq!(pressed(0, 48), None);
        assert_eq!(pressed(48, 32), None);
    }
    #[test]
    fn absent_disabled_and_invalid_declarations() {
        assert_eq!(
            Elevators::default()
                .destination(&mut scene(), [0.5, 1.0, 0.5], Direction::Up)
                .unwrap(),
            None
        );
        let disabled = Elevators::parse(br#"{"enabled":false,"blocks":["elevatorid:elevator_white"],"range":8,"same_color":false,"precision_target":true}"#).unwrap();
        assert_eq!(
            disabled
                .destination(&mut scene(), [0.5, 1.0, 0.5], Direction::Up)
                .unwrap(),
            None
        );
        assert!(Elevators::parse(br#"{"enabled":true,"blocks":["minecraft:stone"],"range":8,"same_color":false,"precision_target":true}"#).is_err());
        assert!(Elevators::parse(b"{}").is_err());
    }
}

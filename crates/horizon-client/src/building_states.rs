//! Conservative saved-state policy for one-item building transactions.
use horizon_world::chunk::BlockState;

/// One inventory item cannot buy a state containing extra materials, stored
/// energy or crop growth. Orientation may differ from default; other state
/// properties must be reproduced by ordinary placement rather than copied.
pub fn material_preserving(state: &BlockState, default: &BlockState) -> bool {
    state.name == default.name
        && state.properties.iter().all(|(key, value)| {
            if default.properties.get(key) == Some(value) {
                return true;
            }
            match key.as_str() {
                "axis" | "facing" | "rotation" | "shape" | "orientation" => true,
                "half" => matches!(value.as_str(), "top" | "bottom"),
                "type" => matches!(value.as_str(), "top" | "bottom"),
                _ => false,
            }
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state(name: &str, key: &str, value: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: [(key.into(), value.into())].into(),
        }
    }
    #[test]
    fn copying_orientation_does_not_buy_extra_material_or_energy() {
        for (name, key, base, extra) in [
            ("minecraft:candle", "candles", "1", "4"),
            ("minecraft:sea_pickle", "pickles", "1", "4"),
            ("minecraft:snow", "layers", "1", "8"),
            ("minecraft:oak_slab", "type", "bottom", "double"),
            ("minecraft:respawn_anchor", "charges", "0", "4"),
            ("minecraft:wheat", "age", "0", "7"),
            ("minecraft:composter", "level", "0", "8"),
        ] {
            let default = state(name, key, base);
            assert!(!material_preserving(&state(name, key, extra), &default));
            assert!(material_preserving(&default, &default));
        }
        assert!(material_preserving(
            &state("minecraft:oak_log", "axis", "x"),
            &state("minecraft:oak_log", "axis", "y")
        ));
        assert!(material_preserving(
            &state("minecraft:oak_stairs", "facing", "west"),
            &state("minecraft:oak_stairs", "facing", "north")
        ));
        assert!(material_preserving(
            &state("minecraft:oak_slab", "type", "top"),
            &state("minecraft:oak_slab", "type", "bottom")
        ));
    }
}

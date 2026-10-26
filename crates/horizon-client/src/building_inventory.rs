//! Pure staging helpers. Actor/world authority and commit-time wear belong to the host.
use crate::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};

/// Ordinary placement cannot safely apply state overrides or block-entity data.
/// Cosmetic metadata remains supported and is preserved on unconsumed stacks.
pub fn supported_material(stack: &ItemStack) -> bool {
    stack.data().is_none_or(|data| {
        data.other("minecraft:block_state").is_none()
            && data.other("minecraft:block_entity_data").is_none()
            && data.container.is_empty()
    })
}

/// Stage one material debit and recovered item credit without modifying input.
/// Only the first 36 slots participate; the tool slot is reserved throughout.
/// Tool slot 40 is the external offhand (game/shield.rs); it reserves no main slot.
/// Replacement matching is by ID; the host must first validate that the selected
/// material stack's components are supported for this placement.
pub fn stage_exchange(
    inventory: &[Option<ItemStack>],
    selected: usize,
    tool_slot: usize,
    replacement: &str,
    recovered: Option<ItemStack>,
    creative: bool,
    max_stack: impl Fn(&str) -> u32,
) -> Option<Vec<Option<ItemStack>>> {
    let n = inventory.len().min(36);
    if (tool_slot >= n && tool_slot != 40) || selected >= n {
        return None;
    }
    let mut staged = inventory.to_vec();
    if creative {
        return Some(staged);
    }
    let order: Vec<_> = std::iter::once(selected)
        .chain((0..n).filter(|&i| i != selected))
        .filter(|&i| i != tool_slot)
        .collect();
    let slot = order.iter().copied().find(|&i| {
        staged[i]
            .as_ref()
            .is_some_and(|s| s.item.as_ref() == replacement && s.count > 0 && supported_material(s))
    })?;
    let stack = staged[slot].as_mut()?;
    stack.count -= 1;
    if stack.count == 0 {
        staged[slot] = None;
    }
    let Some(mut recovered) = recovered else {
        return Some(staged);
    };
    if recovered.count == 0 {
        return Some(staged);
    }
    let max = max_stack(&recovered.item);
    if max == 0 {
        return None;
    }
    for &i in &order {
        if let Some(stack) = &mut staged[i]
            && stack.same_item_same_components(&recovered)
        {
            let moved = max.saturating_sub(stack.count).min(recovered.count);
            stack.count += moved;
            recovered.count -= moved;
            if recovered.count == 0 {
                return Some(staged);
            }
        }
    }
    for &i in &order {
        if staged[i].is_none() {
            let moved = max.min(recovered.count);
            staged[i] = Some(ItemStack {
                count: moved,
                ..recovered.clone()
            });
            recovered.count -= moved;
            if recovered.count == 0 {
                return Some(staged);
            }
        }
    }
    None
}
const CUSTOM: &str = "minecraft:custom_data";
const TOOLS: &str = "horizon:tools";
fn module_ok(module: &str) -> bool {
    (1..=64).contains(&module.len())
        && module
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
/// NBT strings are UTF-16, so a valid value is returned as owned UTF-8.
pub fn tool_data(stack: &ItemStack, module: &str) -> Option<String> {
    if !module_ok(module) {
        return None;
    }
    let Tag::String(value) = stack.data()?.other(CUSTOM)?.get(TOOLS)?.get(module)? else {
        return None;
    };
    let value = String::from_utf16(&value.0).ok()?;
    (value.len() <= 4096).then_some(value)
}
/// Preserve unrelated NBT; reject malformed existing namespace shapes atomically.
pub fn set_tool_data(stack: &mut ItemStack, module: &str, data: &str) -> Result<(), &'static str> {
    if !module_ok(module) {
        return Err("invalid module ID");
    }
    if data.len() > 4096 {
        return Err("tool data exceeds 4096 bytes");
    }
    let mut custom = stack
        .data()
        .and_then(|d| d.other(CUSTOM))
        .cloned()
        .unwrap_or_else(|| Tag::Compound(Default::default()));
    let Tag::Compound(fields) = &mut custom else {
        return Err("custom_data is not a compound");
    };
    let tools = fields
        .entry(JavaString::from(TOOLS))
        .or_insert_with(|| Tag::Compound(Default::default()));
    let Tag::Compound(modules) = tools else {
        return Err("horizon:tools is not a compound");
    };
    if modules
        .get(&JavaString::from(module))
        .is_some_and(|v| !matches!(v, Tag::String(_)))
    {
        return Err("existing module data is not a string");
    }
    modules.insert(JavaString::from(module), Tag::String(data.into()));
    stack.data_mut().set_other(CUSTOM, Some(custom));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inventory() -> Vec<Option<ItemStack>> {
        let mut v = vec![Some(ItemStack::new("minecraft:dirt", 64)); 36];
        v[0] = Some(ItemStack::new("minecraft:stick", 1));
        v
    }
    #[test]
    fn debit_frees_slot_and_failed_credit_is_atomic() {
        let mut inv = inventory();
        inv[1] = Some(ItemStack::new("minecraft:stone", 1));
        let staged = stage_exchange(
            &inv,
            0,
            0,
            "minecraft:stone",
            Some(ItemStack::new("minecraft:glass", 1)),
            false,
            |_| 64,
        )
        .unwrap();
        assert_eq!(staged[1].as_ref().unwrap().item.as_ref(), "minecraft:glass");
        assert_eq!(inv[1].as_ref().unwrap().count, 1);
        inv[1].as_mut().unwrap().count = 2;
        assert!(
            stage_exchange(
                &inv,
                0,
                0,
                "minecraft:stone",
                Some(ItemStack::new("minecraft:glass", 1)),
                false,
                |_| 64
            )
            .is_none()
        );
        assert_eq!(inv[1].as_ref().unwrap().count, 2);
    }
    #[test]
    fn selected_order_metadata_and_tool_reservation() {
        let mut inv = vec![None; 37];
        inv[0] = Some(ItemStack::new("minecraft:stone", 1));
        inv[1] = Some(ItemStack::new("minecraft:stone", 2));
        inv[2] = Some(ItemStack::new("minecraft:stone", 3));
        let mut recovered = ItemStack::new("minecraft:glass", 2);
        set_tool_data(&mut recovered, "a", "kept").unwrap();
        inv[3] = Some(ItemStack::new("minecraft:glass", 63));
        let staged = stage_exchange(
            &inv,
            2,
            0,
            "minecraft:stone",
            Some(recovered),
            false,
            |_| 64,
        )
        .unwrap();
        assert_eq!(staged[2].as_ref().unwrap().count, 2);
        assert_eq!(staged[1].as_ref().unwrap().count, 2);
        assert_eq!(staged[0].as_ref().unwrap().count, 1);
        assert_eq!(staged[3].as_ref().unwrap().count, 63);
        assert_eq!(
            tool_data(staged[4].as_ref().unwrap(), "a").as_deref(),
            Some("kept")
        );
        inv[1] = None;
        inv[2] = None;
        inv[36] = Some(ItemStack::new("minecraft:stone", 64));
        assert!(stage_exchange(&inv, 0, 0, "minecraft:stone", None, false, |_| 64).is_none());
        let staged = stage_exchange(
            &inv,
            0,
            0,
            "minecraft:stone",
            Some(ItemStack::new("minecraft:glass", 64)),
            true,
            |_| 64,
        )
        .unwrap();
        assert_eq!(staged, inv);
    }
    #[test]
    fn module_data_isolated_preserved_and_roundtrips_components() {
        let mut a = ItemStack::new("minecraft:stick", 1);
        a.data_mut()
            .set_other(CUSTOM, Some(Tag::compound([("vanilla", Tag::Int(7))])));
        set_tool_data(&mut a, "one", "日本語").unwrap();
        set_tool_data(&mut a, "two", "second").unwrap();
        let mut b = a.clone();
        set_tool_data(&mut b, "one", "changed").unwrap();
        assert_eq!(tool_data(&a, "one").as_deref(), Some("日本語"));
        assert_eq!(tool_data(&b, "two").as_deref(), Some("second"));
        assert_eq!(
            a.data().unwrap().other(CUSTOM).unwrap().get("vanilla"),
            Some(&Tag::Int(7))
        );
        let before = a.clone();
        assert!(set_tool_data(&mut a, "bad:id", "x").is_err());
        assert!(set_tool_data(&mut a, "one", &"é".repeat(2049)).is_err());
        assert_eq!(a, before);
        for malformed in [
            Tag::Int(1),
            Tag::compound([(TOOLS, Tag::Int(1))]),
            Tag::compound([(TOOLS, Tag::compound([("one", Tag::Int(1))]))]),
        ] {
            a.data_mut().set_other(CUSTOM, Some(malformed));
            let before = a.clone();
            assert!(set_tool_data(&mut a, "one", "x").is_err());
            assert_eq!(a, before);
        }
    }
    #[test]
    fn tool_state_survives_save_and_nbt_wire_codec() {
        use horizon_protocol::{
            buf::{Codec, Reader, Writer},
            item::Slot,
            registry::{Builtin, Dynamic, Ids},
        };
        let mut item = ItemStack::new("minecraft:stick", 1);
        set_tool_data(&mut item, "wand", "mode=vertical 日本語").unwrap();
        let saved = crate::save::item_to_nbt(&item);
        let restored = crate::save::item_from_nbt(&saved).unwrap();
        assert_eq!(tool_data(&restored, "wand"), tool_data(&item, "wand"));
        let builtin = Builtin {
            registries: [
                (
                    "minecraft:item".into(),
                    Ids::from_names(vec!["minecraft:stick".into()]),
                ),
                (
                    "minecraft:data_component_type".into(),
                    Ids::from_names(vec![CUSTOM.into()]),
                ),
            ]
            .into(),
        };
        let wire = crate::lan::guest::stack_to_network(
            |name| builtin.get("minecraft:item").unwrap().id(name).unwrap(),
            |registry, name| builtin.get(registry)?.id(name),
            &Some(item.clone()),
        )
        .unwrap();
        let mut bytes = Writer::new();
        Slot::write(&Some(wire), &mut bytes);
        let mut reader = Reader::new(&bytes.data);
        let decoded = Slot::read(&mut reader).unwrap().unwrap();
        let back = crate::item_sync::from_network(&decoded, &builtin, &Dynamic::default());
        assert_eq!(tool_data(&back, "wand"), tool_data(&item, "wand"));
    }
    #[test]
    fn recovered_merges_selected_first_and_respects_stack_caps() {
        let mut inv = vec![None; 36];
        inv[0] = Some(ItemStack::new("minecraft:stick", 1));
        inv[1] = Some(ItemStack::new("minecraft:stone", 1));
        let mut returned = ItemStack::new("minecraft:glass", 20);
        set_tool_data(&mut returned, "test", "metadata").unwrap();
        inv[2] = Some(ItemStack {
            count: 15,
            ..returned.clone()
        });
        inv[3] = Some(ItemStack {
            count: 14,
            ..returned.clone()
        });
        let one = stage_exchange(
            &inv,
            3,
            0,
            "minecraft:stone",
            Some(ItemStack {
                count: 1,
                ..returned.clone()
            }),
            false,
            |_| 16,
        )
        .unwrap();
        assert_eq!(one[3].as_ref().unwrap().count, 15);
        assert_eq!(one[2].as_ref().unwrap().count, 15);
        let staged =
            stage_exchange(&inv, 3, 0, "minecraft:stone", Some(returned), false, |_| 16).unwrap();
        assert_eq!(staged[3].as_ref().unwrap().count, 16);
        assert_eq!(staged[2].as_ref().unwrap().count, 16);
        assert_eq!(staged[1].as_ref().unwrap().count, 16);
        assert_eq!(staged[4].as_ref().unwrap().count, 1);
        assert_eq!(
            tool_data(staged[1].as_ref().unwrap(), "test").as_deref(),
            Some("metadata")
        );
    }
    #[test]
    fn unsupported_materials_are_skipped_without_loss_and_offhand_is_external() {
        let mut inv = vec![None; 36];
        for (slot, component) in [
            (0, "minecraft:block_state"),
            (1, "minecraft:block_entity_data"),
        ] {
            let mut stack = ItemStack::new("minecraft:stone", 2);
            stack
                .data_mut()
                .set_other(component, Some(Tag::compound([] as [(&str, Tag); 0])));
            inv[slot] = Some(stack);
        }
        let mut container = ItemStack::new("minecraft:stone", 2);
        container
            .data_mut()
            .container
            .push((0, ItemStack::new("minecraft:diamond", 1)));
        inv[2] = Some(container);
        let before = inv.clone();
        assert!(stage_exchange(&inv, 0, 40, "minecraft:stone", None, false, |_| 64).is_none());
        assert_eq!(inv, before);
        let named=crate::item_data::parse_stack(r#"{id:"minecraft:stone",count:2,components:{"minecraft:custom_name":{text:"Named"},"minecraft:lore":[{text:"Lore"}]}}"#).unwrap().unwrap();
        assert!(supported_material(&named));
        inv[3] = Some(named.clone());
        let staged = stage_exchange(&inv, 0, 40, "minecraft:stone", None, false, |_| 64).unwrap();
        assert_eq!(&staged[..3], &inv[..3]);
        assert_eq!(staged[3].as_ref().unwrap().count, 1);
        assert!(
            staged[3]
                .as_ref()
                .unwrap()
                .same_item_same_components(&named)
        );
        for slot in [36, 37, 38, 39, 41, usize::MAX] {
            assert!(
                stage_exchange(&inv, 0, slot, "minecraft:stone", None, false, |_| 64).is_none()
            );
        }
    }
}

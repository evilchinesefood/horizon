//! Authoritative tool proposals from sandboxed server modules. All writes remain here.
#[cfg(feature = "mods")]
use super::Hand;
use super::{Game, Hit, lan::LanHost};
use horizon_client::tool_channel::ClientMessage;

#[cfg(not(feature = "mods"))]
impl Game<'_> {
    pub(in crate::game) fn use_mod_tool(
        &mut self,
        _: Option<&Hit>,
    ) -> horizon_client::Result<bool> {
        Ok(false)
    }
    pub(in crate::game) fn tick_tool_preview(&mut self) {}
    pub(in crate::game) fn local_tool_control(&mut self, _: ClientMessage) {}
    pub(in crate::game) fn guest_tool_control(&mut self, _: &mut LanHost, _: u32, _: &[u8]) {}
}

#[cfg(feature = "mods")]
#[derive(Default)]
pub(super) struct ToolServer {
    sessions: std::collections::HashMap<Option<u32>, Session>,
}
#[cfg(feature = "mods")]
#[derive(Default)]
struct Session {
    registry: Vec<horizon_client::tool_channel::ToolAdvert>,
    sequence: u64,
    control_tick: Option<u64>,
    use_tick: Option<u64>,
    seen: u64,
}

#[cfg(feature = "mods")]
mod run {
    use super::*;
    use horizon_client::{
        Result,
        building_inventory::{set_tool_data, stage_exchange, supported_material, tool_data},
        items::ItemStack,
        tool_channel::{self, ControlAdvert, ServerMessage, ToolAdvert},
    };
    use horizon_core::{geometry::Direction, position::BlockPos};
    use horizon_mods::{
        Face, Material, Placement, ToolAction, ToolActionKind, ToolCell, ToolContext, ToolEvent,
        ToolPos, ToolRegistration, ToolResponse, ToolTarget,
    };
    use horizon_protocol::packets::{configuration::CustomPayload, play::Clientbound};
    use horizon_world::{
        level::{LevelAccess, PlaceContext, Ticks},
        registry::BlockStateId,
    };
    use std::collections::{HashMap, HashSet};
    use winit::keyboard::KeyCode;

    fn coords(p: &ToolPos) -> [i32; 3] {
        [p.x, p.y, p.z]
    }
    fn point(p: [i32; 3]) -> ToolPos {
        ToolPos {
            x: p[0],
            y: p[1],
            z: p[2],
        }
    }
    fn face(n: [i32; 3]) -> Option<Face> {
        Some(match n {
            [1, 0, 0] => Face::East,
            [-1, 0, 0] => Face::West,
            [0, 1, 0] => Face::Up,
            [0, -1, 0] => Face::Down,
            [0, 0, 1] => Face::South,
            [0, 0, -1] => Face::North,
            _ => return None,
        })
    }
    fn direction(f: Face) -> Direction {
        match f {
            Face::East => Direction::East,
            Face::West => Direction::West,
            Face::Up => Direction::Up,
            Face::Down => Direction::Down,
            Face::South => Direction::South,
            Face::North => Direction::North,
        }
    }
    fn normal(f: Face) -> [i32; 3] {
        match f {
            Face::East => [1, 0, 0],
            Face::West => [-1, 0, 0],
            Face::Up => [0, 1, 0],
            Face::Down => [0, -1, 0],
            Face::South => [0, 0, 1],
            Face::North => [0, 0, -1],
        }
    }

    impl Game<'_> {
        fn send_tool_message(&mut self, message: &ServerMessage) -> bool {
            let Ok(data) = tool_channel::encode_server(message) else {
                return false;
            };
            if self.lan_acting.is_some() {
                self.guest_packets
                    .push(Clientbound::CustomPayload(CustomPayload {
                        channel: tool_channel::CHANNEL.into(),
                        data,
                    }));
            } else {
                self.receive_tool_message(&data);
            }
            true
        }
        fn advertise_tools(&mut self) {
            let tools: Vec<_> = self
                .mods
                .mods
                .tool_registrations()
                .map(|r| ToolAdvert {
                    module: r.module_id,
                    generation: r.generation,
                    item: r.tool.item_id,
                    controls: r
                        .tool
                        .controls
                        .into_iter()
                        .map(|c| ControlAdvert {
                            name: c.name,
                            label: c.label,
                            key: c.key,
                        })
                        .collect(),
                })
                .collect();
            let session = self
                .mods
                .tool_server
                .sessions
                .entry(self.lan_acting)
                .or_default();
            session.seen = self.ticks;
            if session.registry != tools
                && self.send_tool_message(&ServerMessage::Registry {
                    tools: tools.clone(),
                })
            {
                let session = self
                    .mods
                    .tool_server
                    .sessions
                    .entry(self.lan_acting)
                    .or_default();
                session.registry = tools;
                session.sequence = 0;
                session.control_tick = None;
            }
        }
        /// Query the authoritative view, never the packet's forced hit.
        fn tool_hit(&self) -> Option<Hit> {
            self.clip_ray(
                self.eye_position()?,
                super::super::view_vector(self.yaw, self.pitch),
                self.reach(),
            )
        }
        fn tool_loaded(&self, p: [i32; 3]) -> bool {
            let (min, max) = if self.dim == super::super::Dim::Overworld {
                (-64, 320)
            } else {
                (0, 256)
            };
            p[0].abs_diff(0) <= 30_000_000
                && p[2].abs_diff(0) <= 30_000_000
                && (min..max).contains(&p[1])
                && self.world.loaded(p[0], p[2])
        }
        /// Only ordinary single-block states whose item can preserve the material.
        fn tool_ordinary(&self, state: BlockStateId) -> bool {
            let facts = self.res.facts().get(state);
            if facts.hardness < 0.0
                || facts.replaceable
                || facts.block_entity
                || facts.waterlogged
                || facts.item.is_empty()
                || facts.item.as_ref() == "minecraft:air"
            {
                return false;
            }
            let registry = &self.pack.assets.registry;
            let Some(s) = registry.state(state) else {
                return false;
            };
            let Some(default) = registry
                .default_state(&s.name)
                .and_then(|id| registry.state(id))
            else {
                return false;
            };
            if !horizon_client::building_states::material_preserving(s, default) {
                return false;
            }
            if s.properties.contains_key("part")
                || s.properties
                    .get("half")
                    .is_some_and(|v| v == "upper" || v == "lower")
                || s.properties.get("type").is_some_and(|v| v == "double")
                || s.properties.get("extended").is_some_and(|v| v == "true")
            {
                return false;
            }
            self.res.items().get(&facts.item).is_some_and(|i| {
                i.block
                    .is_some_and(|b| self.res.facts().get(b).name == facts.name)
            })
        }
        fn tool_harvest_level(&self, state: BlockStateId) -> u32 {
            let name = &self.res.facts().get(state).name;
            let tags = &self.res.items().block_tags;
            for (tag, level) in [
                ("minecraft:needs_diamond_tool", 3),
                ("minecraft:needs_iron_tool", 2),
                ("minecraft:needs_stone_tool", 1),
            ] {
                if tags.get(tag).is_some_and(|set| set.contains(name.as_ref())) {
                    return level;
                }
            }
            0
        }
        fn tool_context(
            &self,
            registration: &ToolRegistration,
            event: ToolEvent,
            action: &str,
            hit: Option<&Hit>,
        ) -> Option<ToolContext> {
            let held = self.held()?;
            if held.item.as_ref() != registration.tool.item_id {
                return None;
            }
            let info = self.res.items().get(&held.item)?;
            let target = hit.and_then(|hit| {
                Some(ToolTarget {
                    position: point(hit.pos),
                    face: face(hit.face)?,
                    state: self.state_string(self.world.block(hit.pos[0], hit.pos[1], hit.pos[2])),
                })
            });
            let mut cells = Vec::new();
            if let Some(target) = &target {
                let n = normal(target.face);
                let axes: Vec<usize> = (0..3).filter(|&i| n[i] == 0).collect();
                let center = coords(&target.position);
                let radius = registration.tool.radius as i32;
                for layer in 0..=1 {
                    for u in -radius..=radius {
                        for v in -radius..=radius {
                            let mut p = center;
                            p[axes[0]] = p[axes[0]].saturating_add(u);
                            p[axes[1]] = p[axes[1]].saturating_add(v);
                            for i in 0..3 {
                                p[i] = p[i].saturating_add(n[i] * layer);
                            }
                            if !self.tool_loaded(p) {
                                continue;
                            }
                            let state = self.world.block(p[0], p[1], p[2]);
                            let facts = self.res.facts().get(state);
                            cells.push(ToolCell {
                                position: point(p),
                                state: self.state_string(state),
                                block_item: (!facts.item.is_empty()
                                    && facts.item.as_ref() != "minecraft:air")
                                    .then(|| facts.item.to_string()),
                                replaceable: facts.replaceable,
                                ordinary: self.tool_ordinary(state),
                                harvest_level: self.tool_harvest_level(state),
                            });
                        }
                    }
                }
            }
            let mut counts: std::collections::BTreeMap<String, u32> =
                std::collections::BTreeMap::new();
            for (slot, stack) in self.inventory.iter().take(36).enumerate() {
                if slot == self.held_slot() {
                    continue;
                }
                if let Some(stack) = stack
                    && supported_material(stack)
                {
                    let count = counts.entry(stack.item.to_string()).or_default();
                    *count = count.saturating_add(stack.count);
                }
            }
            Some(ToolContext {
                event,
                action: action.into(),
                actor: format!(
                    "{:08x}{:08x}{:08x}{:08x}",
                    self.acting_uuid()[0],
                    self.acting_uuid()[1],
                    self.acting_uuid()[2],
                    self.acting_uuid()[3]
                ),
                dimension: self.dim.id().into(),
                item: held.item.to_string(),
                tool_data: tool_data(held, &registration.module_id).unwrap_or_default(),
                damage: held.damage,
                max_damage: (info.max_damage > 0).then_some(info.max_damage),
                creative: self.creative,
                sneaking: self.keys.contains(&KeyCode::ShiftLeft),
                target,
                cells,
                materials: counts
                    .into_iter()
                    .map(|(item, count)| Material { item, count })
                    .collect(),
            })
        }
        /// Resolve ordinary placement against an overlay where an exchange's old block is absent.
        fn resolve_tool_action(
            &mut self,
            action: &ToolAction,
            context: &ToolContext,
        ) -> Option<(BlockStateId, BlockStateId)> {
            let target = context.target.as_ref()?;
            let pos = coords(&action.position);
            if !self.may_build()
                || !self.tool_loaded(pos)
                || !self.border.contains(f64::from(pos[0]), f64::from(pos[2]))
            {
                return None;
            }
            if context
                .cells
                .iter()
                .all(|c| coords(&c.position) != pos || c.state != action.expected_state)
            {
                return None;
            }
            let old = self.world.block(pos[0], pos[1], pos[2]);
            if self.state_string(old) != action.expected_state {
                return None;
            }
            match action.kind {
                ToolActionKind::Place => {
                    if !self.res.facts().get(old).replaceable {
                        return None;
                    }
                }
                ToolActionKind::Exchange => {
                    if !self.tool_ordinary(old) {
                        return None;
                    }
                    let facts = self.res.facts().get(old);
                    if facts.requires_tool
                        && !self.res.items().correct_for_drops(self.held(), &facts.name)
                    {
                        return None;
                    }
                }
            }
            let item = self.res.items().get(&action.item)?;
            let block = self.res.facts().get(item.block?).name.clone();
            let desired = match &action.placement {
                Placement::SavedState(s) => Some(self.parse_state(s).ok()?),
                Placement::PlayerFacing => None,
            };
            if let Some(s) = desired
                && (!self.tool_ordinary(s) || self.res.facts().get(s).name != block)
            {
                return None;
            }
            let p = BlockPos::new(pos[0], pos[1], pos[2]);
            let ctx = PlaceContext {
                clicked_pos: p,
                face: direction(target.face),
                hit: horizon_core::geometry::Vec3::new(
                    f64::from(pos[0]) + 0.5,
                    f64::from(pos[1]) + 0.5,
                    f64::from(pos[2]) + 0.5,
                ),
                inside: false,
                yaw: self.yaw,
                pitch: self.pitch,
                sneaking: context.sneaking,
                item: action.item.clone(),
                can_use_game_master_blocks: self.can_use_game_master_blocks(),
            };
            let air = self.res.facts().air;
            let result = self.with_level(|behaviours, level, _| {
                let mut dry = super::super::remote::DryLevel::new(level);
                if action.kind == ToolActionKind::Exchange {
                    dry.set_raw(p, air);
                }
                let mut ticks = Ticks::default();
                let (actual, resolved) = behaviours.placement_state(&mut dry, &mut ticks, &ctx)?;
                if actual != p {
                    return None;
                }
                let state = desired.unwrap_or(resolved);
                (behaviours.can_survive(&mut dry, &mut ticks, p, state)
                    && dry.is_unobstructed(p, state))
                .then_some(state)
            })?;
            if result == old || !self.tool_ordinary(result) {
                return None;
            }
            Some((old, result))
        }
        fn tool_response(
            &mut self,
            registration: &ToolRegistration,
            context: &ToolContext,
        ) -> Option<ToolResponse> {
            match self
                .mods
                .mods
                .tool_action(&registration.tool.item_id, context)?
            {
                Ok(response) => Some(response),
                Err(error) => {
                    log::warn!("tool {}: {error}", registration.module_id);
                    None
                }
            }
        }
        fn save_tool_state(
            &mut self,
            registration: &ToolRegistration,
            response: &ToolResponse,
        ) -> bool {
            let Some(data) = &response.tool_data else {
                return true;
            };
            let Some(stack) = self.held_mut() else {
                return false;
            };
            stack.item.as_ref() == registration.tool.item_id
                && set_tool_data(stack, &registration.module_id, data).is_ok()
        }
        fn tool_preview(
            &mut self,
            registration: &ToolRegistration,
            context: &ToolContext,
            response: &ToolResponse,
        ) {
            let mut positions = Vec::new();
            let mut inventory = self.inventory.clone();
            for action in &response.actions {
                let Some((old, _)) = self.resolve_tool_action(action, context) else {
                    continue;
                };
                let recovered = (action.kind == ToolActionKind::Exchange)
                    .then(|| ItemStack::new(&self.res.facts().get(old).item, 1));
                let Some(next) = stage_exchange(
                    &inventory,
                    self.selected,
                    self.held_slot(),
                    &action.item,
                    recovered,
                    self.creative,
                    |item| self.res.items().get(item).map_or(64, |i| i.max_stack),
                ) else {
                    continue;
                };
                inventory = next;
                positions.push(coords(&action.position));
            }
            self.send_tool_message(&ServerMessage::Preview {
                module: registration.module_id.clone(),
                generation: registration.generation,
                item: registration.tool.item_id.clone(),
                dimension: self.dim.id().into(),
                positions,
                label: response
                    .message
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(128)
                    .collect(),
            });
        }
        fn commit_tool_actions(
            &mut self,
            registration: &ToolRegistration,
            context: &ToolContext,
            response: &ToolResponse,
        ) -> Result<()> {
            let sampled: HashMap<_, _> = context
                .cells
                .iter()
                .map(|c| (coords(&c.position), c.state.as_str()))
                .collect();
            let mut seen = HashSet::new();
            for action in &response.actions {
                if self
                    .held()
                    .is_none_or(|s| s.item.as_ref() != registration.tool.item_id)
                {
                    break;
                }
                let pos = coords(&action.position);
                if !seen.insert(pos)
                    || sampled
                        .get(&pos)
                        .is_none_or(|state| *state != action.expected_state)
                {
                    continue;
                }
                let Some((old, new)) = self.resolve_tool_action(action, context) else {
                    continue;
                };
                let recovered = (action.kind == ToolActionKind::Exchange)
                    .then(|| ItemStack::new(&self.res.facts().get(old).item, 1));
                let Some(inventory) = stage_exchange(
                    &self.inventory,
                    self.selected,
                    self.held_slot(),
                    &action.item,
                    recovered,
                    self.creative,
                    |item| self.res.items().get(item).map_or(64, |i| i.max_stack),
                ) else {
                    continue;
                };
                let prior = std::mem::replace(&mut self.inventory, inventory);
                let p = BlockPos::new(pos[0], pos[1], pos[2]);
                let wrote = self.with_level(|b, level, ticks| b.set_block(level, ticks, p, new, 3));
                if !wrote {
                    self.inventory = prior;
                    continue;
                }
                self.damage_held(1);
                if action.kind == ToolActionKind::Exchange {
                    self.mod_block_event(pos, old, false);
                }
                self.mod_block_event(pos, new, true);
                self.stats.add("used", &action.item, 1);
                self.trigger_block(
                    "minecraft:placed_block",
                    pos,
                    &ItemStack::new(&action.item, 1),
                );
                self.process_level_events()?;
            }
            Ok(())
        }
        pub(in crate::game) fn use_mod_tool(&mut self, packet_hit: Option<&Hit>) -> Result<bool> {
            let Some(item) = self.held().map(|s| s.item.to_string()) else {
                return Ok(false);
            };
            let Some(registration) = self.mods.mods.tool_registration(&item) else {
                return Ok(false);
            };
            if !self.may_build() {
                return Ok(true);
            }
            let Some(hit) = self.tool_hit() else {
                return Ok(true);
            };
            if packet_hit.is_none_or(|p| p.pos != hit.pos || p.face != hit.face) {
                return Ok(true);
            }
            let session = self
                .mods
                .tool_server
                .sessions
                .entry(self.lan_acting)
                .or_default();
            if session.use_tick == Some(self.ticks) {
                return Ok(true);
            }
            session.use_tick = Some(self.ticks);
            let Some(context) = self.tool_context(&registration, ToolEvent::Use, "", Some(&hit))
            else {
                return Ok(true);
            };
            let Some(response) = self.tool_response(&registration, &context) else {
                return Ok(true);
            };
            if !self.save_tool_state(&registration, &response) {
                return Ok(true);
            }
            self.commit_tool_actions(&registration, &context, &response)?;
            self.swing_arm();
            self.tick_tool_preview_now();
            Ok(true)
        }
        pub(in crate::game) fn tick_tool_preview(&mut self) {
            if self.ticks.is_multiple_of(4) {
                let main = self
                    .inventory
                    .get(self.selected)
                    .and_then(Option::as_ref)
                    .is_some_and(|s| self.mods.mods.tool_registration(&s.item).is_some());
                self.with_hand(
                    if main { Hand::Main } else { Hand::Off },
                    Self::tick_tool_preview_now,
                );
            }
        }
        fn tick_tool_preview_now(&mut self) {
            if self.remote.is_some() {
                return;
            }
            self.mods
                .tool_server
                .sessions
                .retain(|_, s| self.ticks.saturating_sub(s.seen) < 1200);
            self.advertise_tools();
            let Some(item) = self.held().map(|s| s.item.to_string()) else {
                return;
            };
            let Some(registration) = self.mods.mods.tool_registration(&item) else {
                return;
            };
            let hit = self.tool_hit();
            let Some(context) =
                self.tool_context(&registration, ToolEvent::Preview, "", hit.as_ref())
            else {
                return;
            };
            if let Some(response) = self.tool_response(&registration, &context) {
                self.tool_preview(&registration, &context, &response);
            }
        }
        pub(in crate::game) fn local_tool_control(&mut self, message: ClientMessage) {
            let ClientMessage::Control { item, .. } = &message;
            let hand = if self
                .inventory
                .get(self.selected)
                .and_then(Option::as_ref)
                .is_some_and(|s| s.item.as_ref() == item)
            {
                Hand::Main
            } else if self
                .offhand
                .as_ref()
                .is_some_and(|s| s.item.as_ref() == item)
            {
                Hand::Off
            } else {
                return;
            };
            self.with_hand(hand, |g| g.apply_tool_control(message));
        }
        fn apply_tool_control(&mut self, message: ClientMessage) {
            if self.remote.is_some() || !self.may_build() {
                return;
            }
            let ClientMessage::Control {
                module,
                generation,
                item,
                action,
                sequence,
            } = message;
            let Some(registration) = self.mods.mods.tool_registration(&item) else {
                return;
            };
            if registration.module_id != module
                || registration.generation != generation
                || self.held().is_none_or(|s| s.item.as_ref() != item)
            {
                return;
            }
            let session = self
                .mods
                .tool_server
                .sessions
                .entry(self.lan_acting)
                .or_default();
            if sequence <= session.sequence || session.control_tick == Some(self.ticks) {
                return;
            }
            session.sequence = sequence;
            session.control_tick = Some(self.ticks);
            session.seen = self.ticks;
            let hit = self.tool_hit();
            let Some(context) =
                self.tool_context(&registration, ToolEvent::Control, &action, hit.as_ref())
            else {
                return;
            };
            let Some(response) = self.tool_response(&registration, &context) else {
                return;
            };
            if response.actions.is_empty() && self.save_tool_state(&registration, &response) {
                self.tick_tool_preview_now();
            }
        }
        pub(in crate::game) fn guest_tool_control(
            &mut self,
            host: &mut LanHost,
            conn: u32,
            bytes: &[u8],
        ) {
            let Ok(message) = tool_channel::decode_client(bytes) else {
                return;
            };
            self.as_guest(host, conn, |game| {
                game.with_hand(Hand::Main, |g| g.local_tool_control(message))
            });
        }
    }
}

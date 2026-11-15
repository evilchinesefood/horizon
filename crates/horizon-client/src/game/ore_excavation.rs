//! Actor-owned ore jobs; destruction still goes through normal player mining.
use super::{Game, Screen, lan::LanHost};
use horizon_client::{
    items::ItemStack,
    ore_excavation::{self as ore, OreFamily, Plan, Pos},
};
use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound as S};

#[derive(Default)]
pub(super) struct ActorState {
    pub held: bool,
    pub removed: usize,
    job: Option<Job>,
}
struct Job {
    plan: Plan,
    family: OreFamily,
    dimension: String,
    slot: usize,
    tool: ItemStack,
}
impl ActorState {
    pub(super) fn set_held(&mut self, held: bool) {
        self.held = held;
        if !held {
            self.job = None;
        }
    }
    pub(super) fn remaining(&self) -> usize {
        self.job.as_ref().map_or(0, |j| j.plan.remaining())
    }
}
impl Game<'_> {
    pub(super) fn ore_enabled(&self) -> bool {
        self.settings
            .mods
            .installed
            .iter()
            .any(|m| m.id == "oreexcavation" && m.problem.is_none())
            && self
                .mods
                .content_is_enabled("oreexcavation", &self.settings.mods.enabled)
    }
    fn ore_actor_eligible(&self) -> bool {
        self.ore_enabled()
            && !self.creative
            && self.may_build()
            && self
                .player
                .as_ref()
                .is_some_and(|p| p.health.health() > 0.0)
    }
    /// Client input only. Remote authority receives a single boolean, never positions.
    pub(super) fn ore_input(&mut self) {
        let playing = self.screen == Screen::Playing
            && self.title.is_none()
            && self.compass_picker.is_none()
            && self.elevator_edit.is_none()
            && self.sign_edit.is_none()
            && self.book.is_none();
        let held = playing
            && (self.focused || self.control.is_some())
            && self.ore_enabled()
            && self.physical_keys.iter().any(|k| {
                horizon_client::keybinds::key_name(*k) == Some(self.settings.keys.get(ore::KEY))
            });
        if self.remote.is_some() {
            if held != self.ore_intent_sent {
                self.send(S::CustomPayload(CustomPayload {
                    channel: ore::CHANNEL.into(),
                    data: vec![u8::from(held)],
                }));
                self.ore_intent_sent = held;
            }
        } else {
            self.ore_excavation.set_held(held);
        }
    }
    pub(super) fn ore_release(&mut self) {
        self.ore_excavation.set_held(false);
        if self.remote.is_some() && self.ore_intent_sent {
            self.send(S::CustomPayload(CustomPayload {
                channel: ore::CHANNEL.into(),
                data: vec![0],
            }));
        }
        self.ore_intent_sent = false;
    }
    pub(super) fn lan_ore_message(&mut self, host: &mut LanHost, conn: u32, data: &[u8]) {
        let Some(held) = ore::held_intent(data) else {
            return;
        };
        if !host.hub.guests.iter().any(|g| g.conn == conn) {
            return;
        }
        let held = held && self.ore_enabled();
        host.play
            .entry(conn)
            .or_default()
            .ore_excavation
            .set_held(held);
    }
    /// Called only after an ordinary mining seed was confirmed removed.
    pub(super) fn ore_seed(&mut self, seed: Pos, family: Option<OreFamily>) {
        let Some(family) = family else {
            return;
        };
        if !self.ore_excavation.held
            || !self.ore_actor_eligible()
            || self.ore_excavation.job.is_some()
        {
            return;
        }
        let Some(tool) = self.inventory[self.selected].clone() else {
            return;
        };
        let plan = Plan::build(seed, family, |p| {
            if !self.world.loaded(p[0], p[2]) {
                return None;
            }
            ore::ore_family(
                &self
                    .res
                    .facts()
                    .get(self.world.block(p[0], p[1], p[2]))
                    .name,
            )
        });
        self.ore_excavation.removed = 1;
        self.ore_excavation.job = Some(Job {
            plan,
            family,
            dimension: self.dim.id().into(),
            slot: self.selected,
            tool,
        });
    }
    pub(super) fn ore_tick(&mut self) -> super::Result<()> {
        let Some(mut job) = self.ore_excavation.job.take() else {
            return Ok(());
        };
        for _ in 0..ore::BREAKS_PER_TICK {
            if !self.ore_excavation.held
                || !self.ore_actor_eligible()
                || job.dimension != self.dim.id()
                || job.slot != self.selected
                || self.inventory[self.selected]
                    .as_ref()
                    .is_none_or(|tool| !ore::same_tool(tool, &job.tool))
            {
                return Ok(());
            }
            let Some((index, pos)) = job.plan.next_candidate() else {
                return Ok(());
            };
            if !self.world.loaded(pos[0], pos[2]) {
                continue;
            }
            let facts = self
                .res
                .facts()
                .get(self.world.block(pos[0], pos[1], pos[2]));
            if ore::ore_family(&facts.name) != Some(job.family)
                || facts.hardness < 0.0
                || !self
                    .res
                    .items()
                    .correct_for_drops(Some(&job.tool), &facts.name)
            {
                continue;
            }
            // This method returns the existing player_destroy_block outcome and
            // never starts another excavation. The caller already entered this actor.
            if self.with_player_actor(|g| g.break_block_as_player(pos))? {
                job.plan.confirm_removed(index);
                self.ore_excavation.removed += 1;
                let Some(tool) = self.inventory[self.selected].clone() else {
                    return Ok(());
                };
                job.tool = tool;
            }
        }
        if job.plan.remaining() > 0 {
            self.ore_excavation.job = Some(job);
        }
        Ok(())
    }
}

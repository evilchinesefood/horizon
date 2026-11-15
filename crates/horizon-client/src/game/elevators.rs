use super::{Game, lan::LanHost};
use horizon_client::{
    chunk::{HEIGHT, MIN_Y},
    elevator::{Direction, Elevators, World},
    motion::MotionWorld,
};
use horizon_core::geometry::{Aabb, Vec3};

struct LandingWorld<'a, 'b, 'c>(MotionWorld<'a, 'b, 'c>);
impl World for LandingWorld<'_, '_, '_> {
    fn block(&self, [x, y, z]: [i32; 3]) -> Option<&str> {
        if !(MIN_Y..MIN_Y + HEIGHT).contains(&y) || !self.0.scene.loaded(x, z) {
            return None;
        }
        self.0
            .assets
            .registry
            .state(self.0.scene.block(x, y, z))
            .map(|s| s.name.as_str())
    }
    fn can_land(&mut self, at: [f64; 3]) -> super::Result<bool> {
        let bounds = self
            .0
            .player
            .dimensions
            .bounds(Vec3::new(at[0], at[1], at[2]));
        if !horizon_client::elevator::loaded_landing(bounds, |x, z| {
            self.0.scene.chunks.contains_key(&(x, z))
        }) {
            return Ok(false);
        }
        let previous = self.0.player.position;
        self.0.player.position = Vec3::new(at[0], at[1], at[2]);
        let result = self.0.no_player_collision(bounds);
        self.0.player.position = previous;
        result
    }
}

pub(super) fn travel(
    rules: &Elevators,
    motion: MotionWorld<'_, '_, '_>,
    feet: [f64; 3],
    direction: Direction,
) -> super::Result<bool> {
    let mut world = LandingWorld(motion);
    let Some(at) = rules.destination(&mut world, feet, direction)? else {
        return Ok(false);
    };
    let options = match world.0.scene.block_entities.get(&(
        at[0].floor() as i32,
        at[1] as i32 - 1,
        at[2].floor() as i32,
    )) {
        Some(horizon_client::containers::BlockEntity::Elevator(b)) => Some(b.options),
        _ => None,
    };
    let player = &mut world.0.player;
    player.position = Vec3::new(at[0], at[1], at[2]);
    player.velocity = Vec3::ZERO;
    player.fall_distance = 0.0;
    player.ground.on_ground = true;
    player.ground.supporting_block = None;
    player.jumping = false;
    player.teleported = true;
    if let Some(options) = options.filter(|o| o.directional) {
        player.yaw = options.heading.yaw();
        player.pitch = 0.0;
    }
    Ok(true)
}

impl Game<'_> {
    pub(super) fn elevator_entity_collisions(&self) -> Vec<Aabb> {
        self.mobs
            .iter()
            .filter(|m| {
                m.species() == horizon_client::mobs::Species::HappyGhast
                    && m.alive()
                    && !m.is_baby()
            })
            .map(|m| m.bounds())
            .chain(self.boats.iter().map(super::boats::Boat::bounds))
            .collect()
    }

    pub(super) fn guest_elevator(&mut self, host: &mut LanHost, conn: u32) {
        let tick = host.hub.ticks;
        let Some(g) = host.hub.guest_mut(conn) else {
            return;
        };
        let Some(request) = g.elevator_activation.take() else {
            return;
        };
        if !request.grounded
            || g.dead
            || g.health <= 0.0
            || g.spectator()
            || g.sleeping
            || g.vehicle.is_some()
            || g.awaiting_teleport.is_some()
            || request.teleport != g.teleport_counter
            || request.dimension != g.dimension
            || tick.saturating_sub(request.tick) > 2
            || (0..3).any(|i| (request.position[i] - g.position[i]).abs() > 1.5)
            || !self
                .mods
                .content_is_enabled("elevatorid", &self.settings.mods.enabled)
        {
            return;
        }
        let moved = self.as_guest(host, conn, |game| -> super::Result<bool> {
            let entities = game.elevator_entity_collisions();
            let Some(player) = game.player.as_mut() else {
                return Ok(false);
            };
            let moved = travel(
                &game.pack.elevators,
                MotionWorld {
                    entities: &entities,
                    player,
                    scene: &mut game.world,
                    assets: &game.pack.assets,
                    collision_binding: game.collision,
                    resources: game.motion,
                },
                request.position,
                request.direction,
            )?;
            if moved {
                game.yaw = player.yaw;
                game.pitch = player.pitch;
            }
            Ok(moved)
        });
        match moved {
            Some(Ok(true)) => {
                if let Some(g) = host.hub.guest_mut(conn) {
                    g.moves.clear();
                    g.moves_from_ground = true;
                    g.on_ground = true;
                }
            }
            Some(Err(e)) => log::warn!("elevator travel failed: {e}"),
            _ => {}
        }
    }
}

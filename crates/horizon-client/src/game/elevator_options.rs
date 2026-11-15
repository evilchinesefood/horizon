use super::{Game, lan::LanHost};
use horizon_client::{
    containers::BlockEntity,
    elevator_options::{self, Action, ElevatorBlock, ElevatorOptions, Heading, Message},
    hud::{HudBatch, rgba},
    widgets::{Kind, Node},
};
use horizon_protocol::packets::play::{Clientbound as C, Serverbound as S};
use winit::{
    event::{ElementState, KeyEvent},
    keyboard::{KeyCode, PhysicalKey},
};

#[derive(Clone)]
pub(super) struct Editor {
    pub pos: [i32; 3],
    pub dimension: String,
    pub session: u64,
    pub instance_id: u64,
    pub options: ElevatorOptions,
}
impl Game<'_> {
    pub(super) fn elevator_available(&self, pos: [i32; 3]) -> bool {
        self.pack.elevators.is_enabled()
            && self
                .mods
                .content_is_enabled("elevatorid", &self.settings.mods.enabled)
            && self.world.loaded(pos[0], pos[2])
            && elevator_options::is_elevator(
                &self
                    .res
                    .facts()
                    .get(self.world.block(pos[0], pos[1], pos[2]))
                    .name,
            )
    }
    pub(super) fn use_elevator_options(&mut self, pos: [i32; 3]) -> bool {
        if !self.elevator_available(pos)
            || self.spectator()
            || self
                .player
                .as_ref()
                .is_none_or(|p| p.health.health() <= 0.0)
        {
            return false;
        }
        let key = (pos[0], pos[1], pos[2]);
        if !matches!(
            self.world.block_entities.get(&key),
            Some(BlockEntity::Elevator(_))
        ) {
            self.world
                .block_entities
                .insert(key, BlockEntity::Elevator(ElevatorBlock::default()));
        }
        let Some(BlockEntity::Elevator(block)) = self.world.block_entities.get(&key) else {
            return false;
        };
        self.elevator_edit = Some(Editor {
            pos,
            dimension: self.dim.id().into(),
            session: elevator_options::identity(),
            instance_id: block.instance_id,
            options: block.options,
        });
        self.keys.clear();
        self.left = false;
        self.right = false;
        self.capture(false);
        true
    }
    pub(super) fn close_elevator_editor(&mut self) {
        if let Some(edit) = self.elevator_edit.take()
            && self.remote.is_some()
        {
            self.send(S::CustomPayload(
                Message::Action {
                    session: edit.session,
                    action: Action::Close,
                }
                .packet(),
            ));
        }
        self.keys.clear();
        self.left = false;
        self.right = false;
        self.capture(true);
    }
    pub(super) fn lan_open_elevator_editor(&mut self, host: &mut LanHost, conn: u32, edit: Editor) {
        host.hub.send_to(
            conn,
            &C::CustomPayload(
                Message::Open {
                    session: edit.session,
                    pos: edit.pos,
                    state: self
                        .world
                        .block_entities
                        .get(&(edit.pos[0], edit.pos[1], edit.pos[2]))
                        .and_then(|b| {
                            if let BlockEntity::Elevator(b) = b {
                                Some(b.options)
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default(),
                }
                .packet(),
            ),
        );
        host.play.entry(conn).or_default().elevator_edit = Some(edit);
    }
    fn valid_elevator_instance(&self, edit: &Editor) -> bool {
        edit.dimension == self.dim.id()
            && self.elevator_available(edit.pos)
            && matches!(self.world.block_entities.get(&(edit.pos[0],edit.pos[1],edit.pos[2])),Some(BlockEntity::Elevator(b)) if b.instance_id==edit.instance_id)
    }
    pub(super) fn lan_elevator_message(&mut self, host: &mut LanHost, conn: u32, data: &[u8]) {
        let Some(Message::Action { session, action }) = Message::decode(data) else {
            return;
        };
        let Some(edit) = host.play.get(&conn).and_then(|p| p.elevator_edit.clone()) else {
            return;
        };
        if session != edit.session {
            return;
        }
        let valid = host.hub.guest(conn).is_some_and(|g| {
            g.dimension == edit.dimension
                && !g.dead
                && g.health > 0.0
                && !g.spectator()
                && !g.sleeping
                && elevator_options::within_editor_reach(
                    [g.position[0], g.position[1] + g.eye_height(), g.position[2]],
                    g.creative(),
                    edit.pos,
                )
        }) && self.valid_elevator_instance(&edit);
        if !valid || action == Action::Close {
            host.play.entry(conn).or_default().elevator_edit = None;
            host.hub
                .send_to(conn, &C::CustomPayload(Message::Close { session }.packet()));
            return;
        }
        self.apply_elevator_action(&edit, action);
    }
    pub(super) fn tick_guest_elevator_editor(&mut self, host: &mut LanHost, conn: u32) {
        let Some(edit) = host.play.get(&conn).and_then(|p| p.elevator_edit.clone()) else {
            return;
        };
        let valid = host.hub.guest(conn).is_some_and(|g| {
            g.dimension == edit.dimension
                && !g.dead
                && g.health > 0.0
                && !g.spectator()
                && !g.sleeping
                && elevator_options::within_editor_reach(
                    [g.position[0], g.position[1] + g.eye_height(), g.position[2]],
                    g.creative(),
                    edit.pos,
                )
        }) && self.valid_elevator_instance(&edit);
        if !valid {
            host.play.entry(conn).or_default().elevator_edit = None;
            host.hub.send_to(
                conn,
                &C::CustomPayload(
                    Message::Close {
                        session: edit.session,
                    }
                    .packet(),
                ),
            );
        }
    }
    fn apply_elevator_action(&mut self, edit: &Editor, action: Action) {
        if !self.valid_elevator_instance(edit) {
            return;
        }
        let key = (edit.pos[0], edit.pos[1], edit.pos[2]);
        if let Some(BlockEntity::Elevator(b)) = self.world.block_entities.get_mut(&key) {
            b.options.apply(action);
            self.mark_block_entity_updated(key);
        }
    }
    pub(super) fn remote_elevator_message(&mut self, data: &[u8]) {
        if !self.pack.elevators.is_enabled() {
            return;
        }
        match Message::decode(data) {
            Some(Message::Open {
                session,
                pos,
                state,
            }) if self.elevator_available(pos) => {
                self.receive_elevator_state(pos, state);
                self.elevator_edit = Some(Editor {
                    pos,
                    dimension: self.dim.id().into(),
                    session,
                    instance_id: 0,
                    options: state,
                });
                self.keys.clear();
                self.left = false;
                self.right = false;
                self.capture(false);
            }
            Some(Message::State { pos, state }) => self.receive_elevator_state(pos, state),
            Some(Message::Chunk { x, z, entries }) if self.world.chunks.contains_key(&(x, z)) => {
                for (pos, state) in entries {
                    self.receive_elevator_state(pos, state);
                }
            }
            Some(Message::Close { session })
                if self
                    .elevator_edit
                    .as_ref()
                    .is_some_and(|e| e.session == session) =>
            {
                self.elevator_edit = None;
                self.capture(true);
            }
            _ => {}
        }
    }
    fn receive_elevator_state(&mut self, pos: [i32; 3], state: ElevatorOptions) {
        if !self.elevator_available(pos) {
            return;
        }
        if let Some(edit) = self.elevator_edit.as_mut()
            && edit.pos == pos
        {
            edit.options = state;
        }
        let key = (pos[0], pos[1], pos[2]);
        if let Some(BlockEntity::Elevator(block)) = self.world.block_entities.get_mut(&key) {
            block.options = state;
        } else {
            self.world.block_entities.insert(
                key,
                BlockEntity::Elevator(ElevatorBlock {
                    options: state,
                    ..Default::default()
                }),
            );
        }
    }
    pub(super) fn tick_elevator_editor(&mut self) {
        let Some(edit) = self.elevator_edit.as_ref() else {
            return;
        };
        let near = self
            .eye_position()
            .is_some_and(|eye| elevator_options::within_editor_reach(eye, self.creative, edit.pos))
            && self
                .player
                .as_ref()
                .is_some_and(|p| p.health.health() > 0.0);
        if !near
            || edit.dimension != self.dim.id()
            || !self.elevator_available(edit.pos)
            || (self.remote.is_none() && !self.valid_elevator_instance(edit))
        {
            self.close_elevator_editor();
        }
    }
    pub(super) fn elevator_nodes(&self, size: [f32; 2]) -> Vec<Node> {
        let Some(edit) = self.elevator_edit.as_ref() else {
            return Vec::new();
        };
        let (x, y) = (size[0] / 2.0 - 110.0, size[1] / 2.0 - 70.0);
        let mut nodes = vec![Node {
            id: "elevator-directional".into(),
            kind: Kind::Button,
            label: Some(self.translate("screen.elevatorid.elevator.directional", "Directional")),
            rect: [x + 10.0, y + 28.0, 120.0, 20.0],
            enabled: true,
        }];
        if edit.options.directional {
            nodes.push(Node {
                id: "elevator-hide-arrow".into(),
                kind: Kind::Button,
                label: Some(self.translate("screen.elevatorid.elevator.hide_arrow", "Hide arrow")),
                rect: [x + 10.0, y + 55.0, 120.0, 20.0],
                enabled: true,
            });
            let quarter = ((self.yaw / 90.0).round() as i32).rem_euclid(4) as usize;
            for (heading, id, label) in [
                (Heading::North, "north", "N"),
                (Heading::East, "east", "E"),
                (Heading::South, "south", "S"),
                (Heading::West, "west", "W"),
            ] {
                let offsets = [[0.0, 22.0], [-22.0, 0.0], [0.0, -22.0], [22.0, 0.0]];
                let offset = offsets[(heading.index() as usize + 4 - quarter) % 4];
                nodes.push(Node {
                    id: format!("elevator-heading-{id}"),
                    kind: Kind::Button,
                    label: Some(label.into()),
                    rect: [x + 165.0 + offset[0], y + 46.0 + offset[1], 20.0, 20.0],
                    enabled: edit.options.heading != heading,
                });
            }
        }
        nodes.push(Node {
            id: "elevator-done".into(),
            kind: Kind::Button,
            label: Some(self.translate("gui.done", "Done")),
            rect: [x + 60.0, y + 113.0, 100.0, 20.0],
            enabled: true,
        });
        nodes
    }
    pub(super) fn draw_elevator_editor(&self, batch: &mut HudBatch) {
        let Some(edit) = self.elevator_edit.as_ref() else {
            return;
        };
        let atlas = &self.gui.atlas;
        let (x, y) = (batch.width / 2.0 - 110.0, batch.height / 2.0 - 70.0);
        batch.fill(
            atlas,
            0.0,
            0.0,
            batch.width,
            batch.height,
            rgba(0, 0, 0, 100),
        );
        batch.fill(
            atlas,
            x - 1.0,
            y - 1.0,
            222.0,
            142.0,
            rgba(140, 140, 140, 255),
        );
        batch.fill(atlas, x, y, 220.0, 140.0, rgba(48, 48, 48, 255));
        batch.text(
            atlas,
            &self.translate("screen.elevatorid.elevator", "Elevator Options"),
            x + 8.0,
            y + 9.0,
            u32::MAX,
            false,
        );
        for node in self.elevator_nodes([batch.width, batch.height]) {
            if node.enabled {
                horizon_client::widgets::target(node.rect);
            }
            let [bx, by, bw, bh] = node.rect;
            let checked = match node.id.as_str() {
                "elevator-directional" => Some(edit.options.directional),
                "elevator-hide-arrow" => Some(edit.options.hide_arrow),
                _ => None,
            };
            let mut tx = bx + 4.0;
            if let Some(on) = checked {
                batch.fill(atlas, bx, by, 18.0, 18.0, rgba(170, 170, 170, 255));
                batch.fill(atlas, bx + 1.0, by + 1.0, 16.0, 16.0, rgba(25, 25, 25, 255));
                if on {
                    batch.text(atlas, "x", bx + 6.0, by + 5.0, u32::MAX, false);
                }
                tx = bx + 23.0;
            } else if !node.id.starts_with("elevator-heading") {
                batch.sprite_scaled(
                    atlas,
                    if node.enabled {
                        "gui/sprites/widget/button"
                    } else {
                        "gui/sprites/widget/button_disabled"
                    },
                    bx,
                    by,
                    bw,
                    bh,
                    u32::MAX,
                );
            }
            let color = if !node.enabled && node.id.starts_with("elevator-heading") {
                rgba(0, 255, 0, 255)
            } else if node.enabled {
                u32::MAX
            } else {
                rgba(160, 160, 160, 255)
            };
            batch.text(
                atlas,
                node.label.as_deref().unwrap_or(""),
                tx,
                by + 6.0,
                color,
                false,
            );
        }
    }
    pub(super) fn elevator_editor_click(&mut self, size: [u32; 2]) {
        let b = HudBatch::new(size);
        let Some(node) = self
            .elevator_nodes([b.width, b.height])
            .into_iter()
            .find(|n| {
                n.enabled
                    && self.cursor.0 >= n.rect[0]
                    && self.cursor.0 < n.rect[0] + n.rect[2]
                    && self.cursor.1 >= n.rect[1]
                    && self.cursor.1 < n.rect[1] + n.rect[3]
            })
        else {
            return;
        };
        let Some(edit) = self.elevator_edit.clone() else {
            return;
        };
        let action = match node.id.as_str() {
            "elevator-done" => {
                self.close_elevator_editor();
                return;
            }
            "elevator-directional" => Action::Directional(!edit.options.directional),
            "elevator-hide-arrow" => Action::HideArrow(!edit.options.hide_arrow),
            "elevator-heading-north" => Action::Heading(Heading::North),
            "elevator-heading-east" => Action::Heading(Heading::East),
            "elevator-heading-south" => Action::Heading(Heading::South),
            "elevator-heading-west" => Action::Heading(Heading::West),
            _ => return,
        };
        self.audio.play_button_click();
        if self.remote.is_some() {
            self.send(S::CustomPayload(
                Message::Action {
                    session: edit.session,
                    action,
                }
                .packet(),
            ));
        } else {
            self.apply_elevator_action(&edit, action);
            if let Some(BlockEntity::Elevator(b)) =
                self.world
                    .block_entities
                    .get(&(edit.pos[0], edit.pos[1], edit.pos[2]))
                && let Some(e) = self.elevator_edit.as_mut()
            {
                e.options = b.options;
            }
        }
    }
    pub(super) fn elevator_editor_key(&mut self, event: &KeyEvent) {
        if event.state == ElementState::Pressed
            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape))
        {
            self.close_elevator_editor();
        }
        self.keys.clear();
    }
}

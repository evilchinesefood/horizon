use super::{Game, Hand, lan::LanHost};
use horizon_client::{
    compass::{self, CompassKind, Message, Target},
    hud::{HudBatch, rgba},
    widgets::{Kind, Node},
};
use horizon_protocol::packets::play::{Clientbound as C, Serverbound as S};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver},
};
use winit::{
    event::{ElementState, KeyEvent},
    keyboard::{KeyCode, PhysicalKey},
};
static WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}
#[derive(Default)]
pub(super) struct Authority {
    session: u64,
    slot: usize,
    identity: u64,
    dimension: String,
    world: Option<std::sync::Weak<horizon_client::save::SaveDir>>,
    kind: Option<CompassKind>,
    catalog: Vec<String>,
    status: String,
    job: Option<Receiver<Option<(String, [i32; 3])>>>,
}
#[derive(Default)]
pub(super) struct Picker {
    pub session: u64,
    pub catalog: Vec<String>,
    pub status: String,
    pub target: Option<Target>,
    pub filter: String,
    pub selected: Option<String>,
    page: usize,
    slot: usize,
    kind: Option<CompassKind>,
    dimension: String,
    identity: u64,
}
impl Picker {
    fn type_filter(&mut self, text: &str, replace: bool) {
        if replace {
            self.filter.clear();
        }
        if text == "\\b" {
            self.filter.pop();
        } else {
            for c in text.chars().filter(|c| !c.is_control()) {
                if self.filter.chars().count() < 128 {
                    self.filter.push(c);
                }
            }
        }
        self.page = 0;
        self.selected = None;
    }
}
impl Authority {
    fn matches_owner(&self, slot: usize, identity: u64, dimension: &str) -> bool {
        self.session != 0
            && self.slot == slot
            && self.identity == identity
            && self.dimension == dimension
    }
    fn cancel(&mut self) {
        self.job = None;
    }
    fn invalidate(&mut self) -> Option<Message> {
        self.cancel();
        let session = std::mem::take(&mut self.session);
        (session != 0).then_some(Message::Close { session })
    }
    fn valid(&self, game: &Game<'_>) -> bool {
        self.matches_owner(
            game.selected,
            game.stack_in(game.selected)
                .map(compass::state)
                .map_or(0, |s| s.identity),
            game.dim.id(),
        ) && self
            .world
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|world| std::sync::Arc::ptr_eq(&world, &game.save))
            && game.player.as_ref().is_some_and(|p| p.health.health() > 0.)
            && game.stack_in(self.slot).is_some_and(|s| {
                compass::state(s).identity == self.identity
                    && CompassKind::of(&s.item) == self.kind
                    && game.compass_enabled(&s.item)
            })
    }
    fn view(&self, game: &Game<'_>) -> Message {
        Message::View {
            open: false,
            slot: self.slot,
            kind: self.kind.unwrap_or(CompassKind::Biome),
            dimension: self.dimension.clone(),
            identity: self.identity,
            session: self.session,
            catalog: self.catalog.clone(),
            status: self.status.clone(),
            target: game
                .stack_in(self.slot)
                .and_then(|s| compass::state(s).target),
        }
    }
    fn apply(&mut self, game: &mut Game<'_>, msg: Message) -> Option<Message> {
        if matches!(msg, Message::Open) {
            let stack = game.stack_in(game.selected)?;
            if !game.compass_enabled(&stack.item) || game.spectator() {
                return None;
            }
            let kind = CompassKind::of(&stack.item)?;
            let terrain = game.end.locator()?;
            self.cancel();
            self.session = horizon_client::elevator_options::identity();
            self.world = Some(std::sync::Arc::downgrade(&game.save));
            self.slot = game.selected;
            self.dimension = game.dim.id().into();
            self.kind = Some(kind);
            self.catalog = terrain.compass_catalog(kind == CompassKind::Structure);
            self.catalog.sort();
            self.catalog.dedup();
            let mut state = compass::state(game.stack_in(self.slot)?);
            if state.identity == 0 {
                state.identity = compass::stack_identity();
            }
            self.identity = state.identity;
            self.status = if state.target.is_some() {
                "Found"
            } else {
                "Choose a target"
            }
            .into();
            compass::write(game.stack_in_mut(self.slot).as_mut()?, &state);
            let mut view = self.view(game);
            if let Message::View { open, .. } = &mut view {
                *open = true;
            }
            return Some(view);
        }
        if !self.valid(game) {
            return self.invalidate();
        }
        match msg {
            Message::Reset { session } if session == self.session => {
                self.cancel();
                let stack = game.stack_in_mut(self.slot).as_mut()?;
                let mut state = compass::state(stack);
                state.target = None;
                compass::write(stack, &state);
                self.status = "Choose a target".into();
            }
            Message::Search { session, target }
                if session == self.session && self.catalog.contains(&target) =>
            {
                if self.job.is_some() {
                    return None;
                }
                let terrain = game.end.locator()?;
                if WORKERS
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                        (n < 2).then_some(n + 1)
                    })
                    .is_err()
                {
                    self.status = "Search busy; try again".into();
                    return Some(self.view(game));
                }
                let permit = Permit;
                let origin = game.eye_position()?.map(|v| v.floor() as i32);
                let structures = self.kind == Some(CompassKind::Structure);
                let (tx, rx) = mpsc::channel();
                self.job = Some(rx);
                self.status = "Searching".into();
                std::thread::spawn(move || {
                    let _permit = permit;
                    let result = terrain.compass_search(structures, &target, origin);
                    let _ = tx.send(result);
                });
            }
            _ => return None,
        }
        Some(self.view(game))
    }
    fn tick(&mut self, game: &mut Game<'_>) -> Option<Message> {
        if !self.valid(game) {
            return self.invalidate();
        }
        let result = match self.job.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.cancel();
                self.status = "Search failed; try again".into();
                return Some(self.view(game));
            }
        };
        self.cancel();
        let stack = game.stack_in_mut(self.slot).as_mut()?;
        let mut state = compass::state(stack);
        state.target = result.map(|(name, pos)| Target {
            name,
            pos,
            dimension: self.dimension.clone(),
        });
        self.status = if state.target.is_some() {
            "Found"
        } else {
            "Not found within search range"
        }
        .into();
        compass::write(stack, &state);
        Some(self.view(game))
    }
}
impl Game<'_> {
    fn compass_enabled(&self, item: &str) -> bool {
        CompassKind::of(item).is_some_and(|k| {
            self.res.items().get(item).is_some()
                && self
                    .mods
                    .content_is_enabled(k.mod_id(), &self.settings.mods.enabled)
        })
    }
    pub(super) fn use_compass(&mut self) -> bool {
        if self.hand != Hand::Main
            || !self
                .stack_in(self.selected)
                .is_some_and(|s| self.compass_enabled(&s.item))
        {
            return false;
        }
        if self.guest_use.is_none() && self.serving.is_none() {
            self.compass_request(Message::Open);
        }
        true
    }
    fn compass_request(&mut self, msg: Message) {
        if self.remote.is_some() {
            self.send(S::CustomPayload(msg.packet()));
        } else {
            let mut authority = std::mem::take(&mut self.compass_authority);
            if let Some(view) = authority.apply(self, msg) {
                self.compass_view(view);
            }
            self.compass_authority = authority;
        }
    }
    pub(super) fn lan_compass_message(&mut self, host: &mut LanHost, conn: u32, data: &[u8]) {
        let Some(msg) = Message::decode(data) else {
            return;
        };
        if !matches!(
            msg,
            Message::Open | Message::Search { .. } | Message::Reset { .. }
        ) {
            return;
        }
        let mut authority = std::mem::take(&mut host.play.entry(conn).or_default().compass);
        let response = self
            .as_guest(host, conn, |game| authority.apply(game, msg))
            .flatten();
        host.play.entry(conn).or_default().compass = authority;
        if let Some(view) = response {
            host.hub.send_to(conn, &C::CustomPayload(view.packet()));
        }
    }
    pub(super) fn tick_guest_compass(&mut self, host: &mut LanHost, conn: u32) {
        let mut authority = std::mem::take(&mut host.play.entry(conn).or_default().compass);
        let response = self
            .as_guest(host, conn, |game| authority.tick(game))
            .flatten();
        host.play.entry(conn).or_default().compass = authority;
        if let Some(view) = response {
            host.hub.send_to(conn, &C::CustomPayload(view.packet()));
        }
    }
    pub(super) fn tick_compass(&mut self) {
        if self.remote.is_none() {
            let mut authority = std::mem::take(&mut self.compass_authority);
            if let Some(view) = authority.tick(self) {
                self.compass_view(view);
            }
            self.compass_authority = authority;
        }
        if self.compass_picker.as_ref().is_some_and(|p| {
            self.screen != super::Screen::Playing
                || self.selected != p.slot
                || self.dim.id() != p.dimension
                || !self.stack_in(self.selected).is_some_and(|s| {
                    let identity = compass::state(s).identity;
                    CompassKind::of(&s.item) == p.kind
                        && self.compass_enabled(&s.item)
                        && (identity == 0 || identity == p.identity)
                })
        }) {
            self.close_compass();
        }
    }
    pub(super) fn remote_compass_message(&mut self, data: &[u8]) {
        if let Some(msg) = Message::decode(data) {
            self.compass_view(msg);
        }
    }
    fn compass_view(&mut self, msg: Message) {
        if let Message::Close { session } = msg {
            if self
                .compass_picker
                .as_ref()
                .is_some_and(|p| p.session == session)
            {
                self.close_compass();
            }
            return;
        }
        let Message::View {
            open,
            slot,
            kind,
            dimension,
            identity,
            session,
            catalog,
            status,
            target,
        } = msg
        else {
            return;
        };
        if slot != self.selected
            || dimension != self.dim.id()
            || !self.stack_in(slot).is_some_and(|s| {
                let current = compass::state(s).identity;
                self.compass_enabled(&s.item)
                    && CompassKind::of(&s.item) == Some(kind)
                    && (current == 0 || current == identity)
            })
        {
            return;
        }
        if open {
            self.compass_picker = None;
        }
        if let Some(picker) = self.compass_picker.as_mut() {
            if picker.session != session {
                return;
            }
            picker.status = status;
            picker.target = target;
        } else if open {
            self.compass_picker = Some(Picker {
                slot,
                kind: Some(kind),
                dimension,
                identity,
                session,
                catalog,
                status,
                target,
                ..Default::default()
            });
            self.keys.clear();
            self.left = false;
            self.right = false;
            self.capture(false);
        }
    }
    fn close_compass(&mut self) {
        self.compass_picker = None;
        self.keys.clear();
        self.left = false;
        self.right = false;
        self.capture(true);
    }
    pub(super) fn compass_nodes(&self, size: [f32; 2]) -> Vec<Node> {
        let Some(p) = &self.compass_picker else {
            return vec![];
        };
        let x = size[0] / 2. - 150.;
        let y = size[1] / 2. - 115.;
        let node = |id: String, label: String, rect, kind| Node {
            id,
            label: Some(label),
            rect,
            kind,
            enabled: true,
        };
        let mut out = vec![node(
            "compass-filter".into(),
            format!("Filter: {}", p.filter),
            [x + 8., y + 24., 284., 18.],
            Kind::Field,
        )];
        for (i, name) in p
            .catalog
            .iter()
            .filter(|n| n.to_lowercase().contains(&p.filter.to_lowercase()))
            .skip(p.page * 6)
            .take(6)
            .enumerate()
        {
            out.push(node(
                format!("compass-target-{i}"),
                name.clone(),
                [x + 8., y + 46. + i as f32 * 20., 284., 18.],
                Kind::Row,
            ));
        }
        for (i, (id, label)) in [
            ("prev", "<"),
            ("next", ">"),
            ("search", "Search"),
            ("reset", "Reset"),
            ("done", "Done"),
        ]
        .iter()
        .enumerate()
        {
            out.push(node(
                format!("compass-{id}"),
                label.to_string(),
                [x + 8. + i as f32 * 58., y + 202., 54., 20.],
                Kind::Button,
            ));
        }
        if let Some(search) = out.iter_mut().find(|n| n.id == "compass-search") {
            search.enabled = p.selected.is_some() && p.status != "Searching";
        }
        out
    }
    pub(super) fn draw_compass(&self, batch: &mut HudBatch) {
        let atlas = &self.gui.atlas;
        if let Some(p) = &self.compass_picker {
            let x = batch.width / 2. - 150.;
            let y = batch.height / 2. - 115.;
            batch.fill(atlas, x, y, 300., 230., rgba(30, 30, 30, 245));
            batch.text(atlas, "Compass", x + 8., y + 8., u32::MAX, false);
            for n in self.compass_nodes([batch.width, batch.height]) {
                horizon_client::widgets::target(n.rect);
                let selected = n.label.as_ref() == p.selected.as_ref();
                batch.fill(
                    atlas,
                    n.rect[0],
                    n.rect[1],
                    n.rect[2],
                    n.rect[3],
                    if selected {
                        rgba(40, 90, 65, 255)
                    } else {
                        rgba(65, 65, 65, 255)
                    },
                );
                batch.text(
                    atlas,
                    n.label.as_deref().unwrap_or(""),
                    n.rect[0] + 3.,
                    n.rect[1] + 5.,
                    u32::MAX,
                    false,
                );
            }
            batch.text(atlas, &p.status, x + 8., y + 174., u32::MAX, false);
            if let Some(target) = p.target.as_ref()
                && let Some(pos) = self.eye_position()
            {
                let navigation = compass::navigation(target, self.dim.id(), pos);
                let direction_distance = navigation
                    .strip_prefix(&format!("{} | ", target.name))
                    .unwrap_or(&navigation);
                batch.text(atlas, direction_distance, x + 8., y + 188., u32::MAX, false);
            }
        }
        if self.compass_picker.is_none()
            && let Some(target) = self
                .stack_in(self.selected)
                .filter(|s| self.compass_enabled(&s.item))
                .and_then(|s| compass::state(s).target)
            && let Some(pos) = self.eye_position()
        {
            batch.text(
                atlas,
                &compass::navigation(&target, self.dim.id(), pos),
                8.,
                36.,
                u32::MAX,
                true,
            );
        }
    }
    pub(super) fn compass_click(&mut self, size: [u32; 2]) {
        let b = HudBatch::new(size);
        let Some(n) = self
            .compass_nodes([b.width, b.height])
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
        let Some(p) = self.compass_picker.as_mut() else {
            return;
        };
        let session = p.session;
        match n.id.as_str() {
            "compass-done" => self.close_compass(),
            "compass-reset" => self.compass_request(Message::Reset { session }),
            "compass-search" => {
                if let Some(target) = p.selected.clone() {
                    self.compass_request(Message::Search { session, target });
                }
            }
            "compass-prev" => p.page = p.page.saturating_sub(1),
            "compass-next" => {
                let count = p
                    .catalog
                    .iter()
                    .filter(|n| n.to_lowercase().contains(&p.filter.to_lowercase()))
                    .count();
                if (p.page + 1) * 6 < count {
                    p.page += 1;
                }
            }
            id if id.starts_with("compass-target-") => p.selected = n.label,
            _ => {}
        }
    }
    pub(super) fn compass_type(&mut self, text: &str, replace: bool) {
        if let Some(p) = self.compass_picker.as_mut() {
            p.type_filter(text, replace);
        }
        self.keys.clear();
    }
    pub(super) fn compass_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) {
            self.close_compass();
            return;
        }
        if matches!(event.physical_key, PhysicalKey::Code(KeyCode::Backspace)) {
            self.compass_type("\\b", false);
        } else if let Some(text) = &event.text {
            self.compass_type(text, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_guard_rejects_replacement_slot_dimension_and_invalidated_session() {
        let mut a = Authority {
            session: 1,
            slot: 2,
            identity: 77,
            dimension: "overworld".into(),
            ..Default::default()
        };
        assert!(a.matches_owner(2, 77, "overworld"));
        assert!(!a.matches_owner(3, 77, "overworld"));
        assert!(!a.matches_owner(2, 78, "overworld"));
        assert!(!a.matches_owner(2, 77, "nether"));
        a.session = 0;
        assert!(!a.matches_owner(2, 77, "overworld"));
    }
    #[test]
    fn reset_drops_old_result_channel_before_a_new_search() {
        let mut a = Authority::default();
        let (old, rx) = mpsc::channel();
        a.job = Some(rx);
        a.cancel();
        let (new, rx) = mpsc::channel();
        a.job = Some(rx);
        assert!(old.send(Some(("old".into(), [1, 2, 3]))).is_err());
        new.send(Some(("new".into(), [4, 5, 6]))).unwrap();
        assert_eq!(
            a.job.as_ref().unwrap().try_recv().unwrap(),
            Some(("new".into(), [4, 5, 6]))
        );
    }
    #[test]
    fn invalidation_closes_only_the_old_session_once() {
        let mut a = Authority {
            session: 19,
            ..Default::default()
        };
        assert!(matches!(
            a.invalidate(),
            Some(Message::Close { session: 19 })
        ));
        assert!(a.invalidate().is_none());
        assert!(!a.matches_owner(0, 0, ""));
    }
    #[test]
    fn filtering_and_control_typing_share_bounds_and_selection_reset() {
        let mut p = Picker {
            filter: "old".into(),
            page: 2,
            selected: Some("minecraft:plains".into()),
            ..Default::default()
        };
        p.type_filter("plains", true);
        assert_eq!(p.filter, "plains");
        assert_eq!(p.page, 0);
        assert!(p.selected.is_none());
        p.type_filter("\\b", false);
        assert_eq!(p.filter, "plain");
        p.type_filter(&"x".repeat(200), true);
        assert_eq!(p.filter.chars().count(), 128);
    }
}

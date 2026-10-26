//! Native client half of the generic tool channel; servers retain action authority.
use super::{Game, Screen};
pub(super) use horizon_client::tool_channel::ToolClient;
use horizon_client::{
    gizmos,
    hud::HudBatch,
    keybinds,
    tool_channel::{self, ServerMessage},
};
use std::time::Instant;
use winit::keyboard::KeyCode;

impl Game<'_> {
    fn observe_tool_context(&mut self) {
        let held = self
            .stack_in(self.selected)
            .filter(|s| self.mods.tool_client.claims_item(&s.item))
            .or_else(|| {
                self.offhand
                    .as_ref()
                    .filter(|s| self.mods.tool_client.claims_item(&s.item))
            })
            .map(|s| s.item.to_string());
        self.mods.tool_client.observe(
            held.as_deref(),
            self.dim.id(),
            self.screen == Screen::Playing && !self.ui.hide_gui,
        );
    }
    pub(super) fn receive_tool_message(&mut self, bytes: &[u8]) {
        self.observe_tool_context();
        if let Ok(message) = tool_channel::decode_server(bytes) {
            self.mods.tool_client.receive(message, Instant::now());
        }
    }
    /// Call with the physical key, on non-repeated keydown, before vanilla handling.
    pub(super) fn tool_key(&mut self, key: KeyCode) -> bool {
        self.observe_tool_context();
        let Some(key) = keybinds::key_name(key) else {
            return false;
        };
        let Some(message) = self.mods.tool_client.control(key) else {
            return false;
        };
        if self.remote.is_some() {
            if let Ok(data) = tool_channel::encode_client(&message) {
                self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                    horizon_protocol::packets::configuration::CustomPayload {
                        channel: tool_channel::CHANNEL.into(),
                        data,
                    },
                ));
            }
        } else {
            self.local_tool_control(message);
        }
        true
    }
    /// Current server status above the hotbar; observes the same hand as controls.
    pub(super) fn draw_tool_hud(&mut self, batch: &mut HudBatch) {
        self.observe_tool_context();
        let Some(ServerMessage::Preview { label, .. }) =
            self.mods.tool_client.preview(Instant::now())
        else {
            return;
        };
        let atlas = &self.gui.atlas;
        let mut text: String = label.chars().take(128).collect();
        let available = (batch.width - 16.0).max(0.0);
        while !text.is_empty() && HudBatch::text_width(atlas, &text) > available {
            text.pop();
        }
        if text.is_empty() || batch.height < 90.0 {
            return;
        }
        let width = HudBatch::text_width(atlas, &text);
        batch.text(
            atlas,
            &text,
            (batch.width - width) / 2.0,
            batch.height - 81.0,
            u32::MAX,
            true,
        );
    }
    /// Append to depth-tested world lines before gizmos::camera_relative.
    pub(super) fn tool_preview_lines(&mut self) -> Vec<gizmos::Line> {
        self.observe_tool_context();
        let mut gizmos = gizmos::Gizmos::default();
        if let Some(ServerMessage::Preview { positions, .. }) =
            self.mods.tool_client.preview(Instant::now())
        {
            for pos in positions {
                let min = pos.map(f64::from);
                gizmos.cuboid(min, min.map(|v| v + 1.0), 0xFF40_FF80, 2.0);
            }
        }
        gizmos.lines
    }
}

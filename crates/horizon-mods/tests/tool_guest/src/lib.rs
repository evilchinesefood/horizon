use horizon_mod_sdk::tools::*;
struct Fixture;
impl ToolMod for Fixture {
    fn init() -> Result<Self, String> {
        Ok(Self)
    }
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            item_id: "fixture:tool".into(),
            radius: 2,
            max_actions: 4,
            controls: [("state", "r"), ("duplicate", "d"), ("trap", "t")]
                .into_iter()
                .map(|(name, key)| Control {
                    name: name.into(),
                    label: name.into(),
                    key: format!("key.keyboard.{key}"),
                })
                .collect(),
        }]
    }
    fn action(&mut self, c: &ToolContext) -> Result<ToolResponse, String> {
        if c.action == "trap" {
            panic!("fixture trap");
        }
        let mut r = ToolResponse {
            tool_data: None,
            message: Some(c.actor.clone()),
            actions: vec![],
        };
        if c.action == "state" {
            r.tool_data = Some("{\"mode\":1}".into());
        }
        if c.event == ToolEvent::Use || c.action == "duplicate" {
            let a = ToolAction {
                kind: ToolActionKind::Place,
                position: ToolPos { x: 0, y: 1, z: 0 },
                expected_state: "minecraft:air".into(),
                item: "minecraft:stone".into(),
                placement: Placement::SavedState("minecraft:stone".into()),
            };
            r.actions.push(a.clone());
            if c.action == "duplicate" {
                r.actions.push(a);
            }
        }
        Ok(r)
    }
}
horizon_mod_sdk::export_tool_mod!(Fixture);

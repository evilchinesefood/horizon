//! Validation for the optional server tool protocol. No mutation authority.
pub use crate::runtime::tool_bindings::horizon::mods::tool_actions::*;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct ToolRegistration {
    pub module_id: String,
    pub generation: u64,
    pub tool: Tool,
}
pub fn tool_limits() -> ToolLimits {
    ToolLimits {
        max_tools: 64,
        max_controls: 16,
        max_actions: 4096,
        max_radius: 32,
        max_cells: 8450,
    }
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.chars().any(char::is_control)
}
fn id(s: &str) -> bool {
    let Some((namespace, path)) = s.split_once(':') else {
        return false;
    };
    !namespace.is_empty()
        && !path.is_empty()
        && s.len() <= 128
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-/".contains(&b))
}
fn state(s: &str) -> bool {
    !s.is_empty() && text(s, 1024)
}
fn data(s: &str) -> bool {
    s.len() <= 4096 && (s.is_empty() || serde_json::from_str::<serde_json::Value>(s).is_ok())
}
pub(crate) fn validate_tools(tools: &[Tool], taken: &HashSet<String>) -> Result<(), String> {
    let lim = tool_limits();
    if tools.len() > lim.max_tools as usize {
        return Err("too many registered tools".into());
    }
    let mut items = HashSet::new();
    for tool in tools {
        if !id(&tool.item_id) || !items.insert(&tool.item_id) || taken.contains(&tool.item_id) {
            return Err("invalid, duplicate or already claimed tool item".into());
        }
        if tool.radius > lim.max_radius
            || tool.max_actions == 0
            || tool.max_actions > lim.max_actions
            || tool.controls.len() > lim.max_controls as usize
        {
            return Err("tool registration exceeds limits".into());
        }
        let mut controls = HashSet::new();
        let mut keys = HashSet::new();
        for c in &tool.controls {
            if c.name.is_empty()
                || !c
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
                || !text(&c.name, 64)
                || !controls.insert(&c.name)
                || c.label.is_empty()
                || !text(&c.label, 128)
                || !c.key.strip_prefix("key.keyboard.").is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.')
                })
                || !text(&c.key, 128)
                || !keys.insert(&c.key)
            {
                return Err("invalid or duplicate tool control".into());
            }
        }
    }
    Ok(())
}
/// Conservative JSON wire budget: module IDs are at most 64 ASCII bytes,
/// generations at most 20 digits, and validated text needs at most 2x escaping.
/// Fixed allowances include field names, punctuation and collection delimiters.
pub(crate) fn validate_registry<'a>(tools: impl Iterator<Item = &'a Tool>) -> Result<(), String> {
    let mut count = 0usize;
    let mut bytes = 64usize;
    for tool in tools {
        count += 1;
        bytes += 256 + 2 * tool.item_id.len();
        for control in &tool.controls {
            bytes += 128 + 2 * (control.name.len() + control.label.len() + control.key.len());
        }
        if count > 256 || bytes > 100 * 1024 {
            return Err("aggregate tool registry exceeds wire limits".into());
        }
    }
    Ok(())
}
pub(crate) fn validate_context(tool: &Tool, c: &ToolContext) -> Result<(), String> {
    if c.item != tool.item_id
        || !id(&c.dimension)
        || c.actor.is_empty()
        || !text(&c.actor, 64)
        || !text(&c.action, 64)
        || !data(&c.tool_data)
        || c.cells.len() > tool_limits().max_cells as usize
        || c.materials.len() > 256
    {
        return Err("invalid tool context bounds".into());
    }
    if c.event == ToolEvent::Control && !tool.controls.iter().any(|v| v.name == c.action) {
        return Err("unregistered tool control".into());
    }
    if c.target.as_ref().is_some_and(|t| !state(&t.state))
        || c.cells
            .iter()
            .any(|v| !state(&v.state) || v.block_item.as_ref().is_some_and(|v| !id(v)))
        || c.materials.iter().any(|v| !id(&v.item))
    {
        return Err("invalid tool snapshot".into());
    }
    Ok(())
}
pub(crate) fn validate_response(
    tool: &Tool,
    context: &ToolContext,
    r: &ToolResponse,
) -> Result<(), String> {
    let max_actions = if context.creative {
        tool.max_actions
    } else {
        context.max_damage.map_or(tool.max_actions, |max| {
            tool.max_actions.min(max.saturating_sub(context.damage))
        })
    };
    if r.actions.len() > max_actions as usize
        || r.tool_data.as_ref().is_some_and(|s| !data(s))
        || r.message.as_ref().is_some_and(|s| !text(s, 1024))
    {
        return Err("tool response exceeds limits".into());
    }
    if context.event == ToolEvent::Preview && r.tool_data.is_some() {
        return Err("preview cannot change tool state".into());
    }
    let mut positions = HashSet::new();
    for a in &r.actions {
        if !positions.insert((a.position.x, a.position.y, a.position.z))
            || !state(&a.expected_state)
            || !id(&a.item)
            || matches!(&a.placement, Placement::SavedState(s) if !state(s))
        {
            return Err("invalid or duplicate tool action".into());
        }
        let Some(target) = &context.target else {
            return Err("tool action requires a target".into());
        };
        let radius = tool.radius.saturating_add(1);
        if a.position.x.abs_diff(target.position.x) > radius
            || a.position.y.abs_diff(target.position.y) > radius
            || a.position.z.abs_diff(target.position.z) > radius
        {
            return Err("tool action lies outside registered radius".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tool() -> Tool {
        Tool {
            item_id: "example:wand".into(),
            radius: 2,
            max_actions: 5,
            controls: vec![Control {
                name: "mode".into(),
                label: "Mode".into(),
                key: "key.keyboard.m".into(),
            }],
        }
    }
    fn context() -> ToolContext {
        ToolContext {
            event: ToolEvent::Use,
            action: String::new(),
            actor: "actor".into(),
            dimension: "minecraft:overworld".into(),
            item: "example:wand".into(),
            tool_data: "{}".into(),
            damage: 0,
            max_damage: Some(10),
            creative: false,
            sneaking: false,
            target: Some(ToolTarget {
                position: ToolPos { x: 0, y: 0, z: 0 },
                face: Face::Up,
                state: "minecraft:stone".into(),
            }),
            cells: vec![],
            materials: vec![],
        }
    }
    fn response() -> ToolResponse {
        ToolResponse {
            tool_data: None,
            message: None,
            actions: vec![ToolAction {
                kind: ToolActionKind::Place,
                position: ToolPos { x: 0, y: 1, z: 0 },
                expected_state: "minecraft:air".into(),
                item: "minecraft:stone".into(),
                placement: Placement::PlayerFacing,
            }],
        }
    }
    #[test]
    fn registration_bounds_and_ownership() {
        let base = tool();
        assert!(validate_tools(std::slice::from_ref(&base), &HashSet::new()).is_ok());
        for invalid in [
            Tool {
                radius: 33,
                ..base.clone()
            },
            Tool {
                max_actions: 4097,
                ..base.clone()
            },
            Tool {
                max_actions: 0,
                ..base.clone()
            },
            Tool {
                controls: vec![base.controls[0].clone(); 2],
                ..base.clone()
            },
        ] {
            assert!(validate_tools(&[invalid], &HashSet::new()).is_err());
        }
        assert!(validate_tools(&vec![base.clone(); 65], &HashSet::new()).is_err());
        assert!(validate_tools(&[base.clone(), base.clone()], &HashSet::new()).is_err());
        assert!(
            validate_tools(
                std::slice::from_ref(&base),
                &HashSet::from([base.item_id.clone()])
            )
            .is_err()
        );
    }
    #[test]
    fn controls_require_distinct_keys() {
        let mut t = tool();
        let mut other = t.controls[0].clone();
        other.name = "other".into();
        t.controls.push(other);
        assert!(validate_tools(&[t], &HashSet::new()).is_err());
    }
    #[test]
    fn aggregate_registry_is_bounded_by_count_and_wire_size() {
        let mut t = tool();
        t.controls.clear();
        assert!(validate_registry(std::iter::repeat_n(&t, 256)).is_ok());
        assert!(validate_registry(std::iter::repeat_n(&t, 257)).is_err());
        let mut large = tool();
        large.controls = (0..16)
            .map(|i| Control {
                name: format!("control{i}"),
                label: "\"".repeat(128),
                key: format!("key.keyboard.{i}"),
            })
            .collect();
        assert!(validate_tools(std::slice::from_ref(&large), &HashSet::new()).is_ok());
        assert!(validate_registry(std::iter::repeat_n(&large, 32)).is_err());
    }
    #[test]
    fn proposals_respect_remaining_durability() {
        let t = tool();
        let mut c = context();
        let mut r = response();
        c.damage = 9;
        assert!(validate_response(&t, &c, &r).is_ok());
        let mut second = r.actions[0].clone();
        second.position.x = 1;
        r.actions.push(second);
        for event in [ToolEvent::Preview, ToolEvent::Use] {
            c.event = event;
            assert!(validate_response(&t, &c, &r).is_err());
        }
        c.creative = true;
        assert!(validate_response(&t, &c, &r).is_ok());
        c.creative = false;
        c.max_damage = None;
        assert!(validate_response(&t, &c, &r).is_ok());
        c.max_damage = Some(10);
        c.damage = 11;
        r.actions.truncate(1);
        assert!(validate_response(&t, &c, &r).is_err());
    }
    #[test]
    fn context_is_bounded_and_control_must_be_registered() {
        let tool = tool();
        let mut c = context();
        assert!(validate_context(&tool, &c).is_ok());
        c.event = ToolEvent::Control;
        c.action = "unknown".into();
        assert!(validate_context(&tool, &c).is_err());
        c.action = "mode".into();
        assert!(validate_context(&tool, &c).is_ok());
        c.tool_data = "[".into();
        assert!(validate_context(&tool, &c).is_err());
        c.tool_data = format!("\"{}\"", "x".repeat(4096));
        assert!(validate_context(&tool, &c).is_err());
    }
    #[test]
    fn proposals_reject_duplicate_excess_remote_and_preview_state() {
        let t = tool();
        let mut c = context();
        let mut r = response();
        assert!(validate_response(&t, &c, &r).is_ok());
        r.actions.push(r.actions[0].clone());
        assert!(validate_response(&t, &c, &r).is_err());
        r = response();
        r.actions[0].position.x = i32::MIN;
        assert!(validate_response(&t, &c, &r).is_err());
        r = response();
        r.tool_data = Some("{}".into());
        c.event = ToolEvent::Preview;
        assert!(validate_response(&t, &c, &r).is_err());
        r.tool_data = None;
        assert!(validate_response(&t, &c, &r).is_ok());
        c.target = None;
        assert!(validate_response(&t, &c, &r).is_err());
    }
}

#![cfg(all(feature = "runtime", feature = "pack"))]
//! Real tool guest: additive exports, capability ownership and bounded proposals.
use horizon_mods::{manifest::componentize, *};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
fn wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = root.join("target/tool-guest-test");
        assert!(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "--offline",
                    "--release",
                    "--target",
                    "wasm32-unknown-unknown",
                    "--manifest-path",
                    "crates/horizon-mods/tests/tool_guest/Cargo.toml",
                    "--target-dir"
                ])
                .arg(&target)
                .status()
                .unwrap()
                .success()
        );
        componentize(
            &std::fs::read(
                target.join("wasm32-unknown-unknown/release/horizon_mods_tool_guest.wasm"),
            )
            .unwrap(),
        )
        .unwrap()
    })
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "horizon-tool-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn package(&self, id: &str, capability: bool) -> PathBuf {
        let caps = if capability {
            "\"tool-actions\",\"players\""
        } else {
            ""
        };
        let manifest = format!(
            "id={id:?}\nname={id:?}\nversion=\"1\"\napi=\"0.3\"\nside=\"server\"\ncapabilities=[{caps}]\nexperimental=[\"tool-actions\"]\n"
        );
        let p = self.0.join(format!("{id}.hmod"));
        std::fs::write(&p, Package::write(&manifest, wasm()).unwrap()).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn context() -> ToolContext {
    ToolContext {
        event: ToolEvent::Use,
        action: String::new(),
        actor: "actor-a".into(),
        dimension: "minecraft:overworld".into(),
        item: "fixture:tool".into(),
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
#[test]
fn real_guest_owns_tools_and_returns_actor_specific_proposals() {
    let f = Fixture::new();
    let p = f.package("tools", true);
    let mut mods = Mods::load_for(Runs::Server, &[p], |_| true, |_| false);
    assert_eq!(mods.entries()[0].status, Status::Running);
    let r = mods.tool_registration("fixture:tool").unwrap();
    assert_eq!(r.module_id, "tools");
    assert!(r.generation > 0);
    let result = mods
        .tool_action("fixture:tool", &context())
        .unwrap()
        .unwrap();
    assert_eq!(result.actions.len(), 1);
    assert_eq!(result.message.as_deref(), Some("actor-a"));
    assert!(mods.tool_action("other:tool", &context()).is_none());
    let mut c = context();
    c.actor = "actor-b".into();
    assert_eq!(
        mods.tool_action("fixture:tool", &c)
            .unwrap()
            .unwrap()
            .message
            .as_deref(),
        Some("actor-b")
    );
    let imports = api_level::imports(wasm()).unwrap();
    assert!(imports.contains(&"tool-actions.limits".into()));
    assert_eq!(api_level::needed(imports.iter().map(String::as_str)).0, 4);
}
#[test]
fn duplicate_ownership_and_missing_capability_do_not_run() {
    let f = Fixture::new();
    let mods = Mods::load_for(
        Runs::Server,
        &[f.package("first", true), f.package("second", true)],
        |_| true,
        |_| false,
    );
    assert_eq!(mods.tool_registrations().count(), 1);
    assert!(matches!(mods.entries()[1].status, Status::Failed(_)));
    let denied = Mods::load_for(
        Runs::Server,
        &[f.package("denied", false)],
        |_| true,
        |_| false,
    );
    assert!(matches!(denied.entries()[0].status, Status::Failed(_)));
    assert!(denied.tool_registration("fixture:tool").is_none());
}
#[test]
fn invalid_proposals_reject_and_trap_removes_registration() {
    let f = Fixture::new();
    let p = f.package("tools", true);
    let mut mods = Mods::load_for(Runs::Server, std::slice::from_ref(&p), |_| true, |_| false);
    let mut c = context();
    c.event = ToolEvent::Control;
    c.action = "duplicate".into();
    assert!(mods.tool_action("fixture:tool", &c).unwrap().is_err());
    c.action = "state".into();
    assert!(
        mods.tool_action("fixture:tool", &c)
            .unwrap()
            .unwrap()
            .tool_data
            .is_some()
    );
    c.event = ToolEvent::Preview;
    assert!(mods.tool_action("fixture:tool", &c).unwrap().is_err());
    c.event = ToolEvent::Control;
    c.action = "trap".into();
    assert!(mods.tool_action("fixture:tool", &c).unwrap().is_err());
    assert!(mods.tool_registration("fixture:tool").is_none());
    let reloaded = Mods::load_for(Runs::Server, &[p], |_| true, |_| false);
    assert!(reloaded.tool_registration("fixture:tool").is_some());
}

#[test]
fn server_tool_package_can_supply_static_item_content() {
    let f = Fixture::new();
    let manifest = r#"id="fixture"
name="Tool and content"
version="1"
api="0.3"
side="server"
capabilities=["registries","tool-actions","players"]
experimental=["tool-actions"]
"#;
    let item = br#"{"template":"minecraft:stick","max_stack_size":1,"max_damage":10}"#;
    let path = f.0.join("content-tool.hmod");
    std::fs::write(
        &path,
        Package::write_files(
            manifest,
            Some(wasm()),
            &[("data/fixture/horizon/item/tool.json", item.as_slice())],
        )
        .unwrap(),
    )
    .unwrap();
    let package = Package::read(&path).unwrap();
    let content = horizon_mods::content::Content::read(&path, &package.manifest).unwrap();
    assert_eq!(
        content.file("data/fixture/horizon/item/tool.json").unwrap(),
        item
    );
    let mods = Mods::load_for(Runs::Server, &[path], |_| true, |_| false);
    assert_eq!(mods.entries()[0].status, Status::Running);
    assert!(mods.tool_registration("fixture:tool").is_some());
    assert!(
        Manifest::parse(
            &manifest
                .replace("api=\"0.3\"", "api=\"0.2\"")
                .replace("side=\"server\"", "side=\"client\"")
                .replace(
                    "[\"registries\",\"tool-actions\",\"players\"]",
                    "[\"registries\"]"
                )
        )
        .is_ok()
    );
}

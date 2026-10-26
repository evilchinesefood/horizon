//! Server-only tools: bounded proposals, never raw world or inventory writes.
//! Manifest: api="0.3", side="server", capabilities=["tool-actions", "players"],
//! experimental=["tool-actions"]. The players import also supplies the unchanged
//! legacy server-event parameter types.
wit_bindgen::generate!({
    path: "wit",
    world: "tool-host",
    pub_export_macro: true,
    export_macro_name: "__export_horizon_tool_mod",
    default_bindings_module: "horizon_mod_sdk::tools",
});
pub use crate::server::{
    __Guest as __ServerGuest, BlockPos as __BlockPos, Registration as __Registration, players,
};
pub use crate::{log, storage};
pub use exports::horizon::mods::tool_events::Guest as __ToolGuest;
pub use horizon::mods::tool_actions::*;

pub trait ToolMod: Sized + 'static {
    fn init() -> Result<Self, String>;
    fn tools(&self) -> Vec<Tool>;
    fn action(&mut self, context: &ToolContext) -> Result<ToolResponse, String>;
}

#[macro_export]
macro_rules! export_tool_mod {
    ($t:ty) => {
        const _: () = {
            use ::std::cell::RefCell;
            use $crate::tools::{ToolMod, Tool, ToolContext, ToolResponse};
            ::std::thread_local! { static STATE: RefCell<Option<$t>> = const { RefCell::new(None) }; }
            struct Export;
            impl $crate::tools::__ServerGuest for Export {
                fn init() -> Result<$crate::tools::__Registration, String> {
                    let _ = $crate::tools::limits();
                    let m = <$t as ToolMod>::init()?;
                    STATE.with(|s| *s.borrow_mut() = Some(m));
                    Ok($crate::tools::__Registration { commands: vec![], tick: false, joins: false, chat: false, blocks: false })
                }
                fn tick(_: u64) {}
                fn player_joined(_: $crate::tools::players::Player) {}
                fn player_left(_: $crate::tools::players::Player) {}
                fn chat(_: $crate::tools::players::Player, _: String) {}
                fn command(_: $crate::tools::players::Player, _: String, _: String) -> Result<Option<String>, String> { Ok(None) }
                fn block_broken(_: $crate::tools::players::Player, _: $crate::tools::__BlockPos, _: String) {}
                fn block_placed(_: $crate::tools::players::Player, _: $crate::tools::__BlockPos, _: String) {}
            }
            impl $crate::tools::__ToolGuest for Export {
                fn register() -> Result<Vec<Tool>, String> {
                    STATE.with(|s| s.borrow().as_ref().map(ToolMod::tools).ok_or_else(|| "tool module is not initialized".into()))
                }
                fn action(context: ToolContext) -> Result<ToolResponse, String> {
                    STATE.with(|s| s.borrow_mut().as_mut().ok_or_else(|| "tool module is not initialized".to_string())?.action(&context))
                }
            }
            #[cfg(target_arch = "wasm32")]
            $crate::tools::__export_horizon_tool_mod!(Export);
            #[cfg(target_arch = "wasm32")]
            $crate::server::__export_horizon_server_mod!(Export);
            #[cfg(not(target_arch = "wasm32"))]
            let _ = <Export as $crate::tools::__ToolGuest>::register;
        };
    };
}
pub use crate::export_tool_mod;

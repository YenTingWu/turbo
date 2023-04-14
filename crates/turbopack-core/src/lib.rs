#![feature(async_closure)]
#![feature(min_specialization)]
#![feature(option_get_or_insert_default)]
#![feature(type_alias_impl_trait)]
#![feature(assert_matches)]
#![feature(lint_reasons)]
#![feature(async_fn_in_trait)]
#![feature(arbitrary_self_types)]

pub mod asset;
pub mod changed;
pub mod chunk;
pub mod code_builder;
pub mod compile_time_info;
pub mod context;
pub mod environment;
pub mod error;
pub mod ident;
pub mod introspect;
pub mod issue;
pub mod reference;
pub mod reference_type;
pub mod resolve;
pub mod server_fs;
pub mod source_asset;
pub mod source_map;
pub mod source_pos;
pub mod source_transform;
pub mod target;
mod utils;
pub mod version;
pub mod virtual_asset;

pub const PROJECT_FILESYSTEM_NAME: &str = "project";
pub const SOURCE_MAP_ROOT_NAME: &str = "turbopack";

pub fn register() {
    turbo_tasks::register();
    turbo_tasks_fs::register();
    include!(concat!(env!("OUT_DIR"), "/register.rs"));
}

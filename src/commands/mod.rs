/// Cross-cutting deps threaded into every command. Borrows — `main::run` owns
/// config + db on the stack for the duration of a command call.
pub struct Ctx<'a> {
    pub config: &'a crate::config::GroveConfig,
    pub db: &'a crate::db::Db,
    pub json_mode: bool,
    pub verbose: bool,
}

pub mod add;
pub mod attach;
pub mod close;
pub mod init;
pub mod list;
pub mod read;
pub mod register;
pub mod repos;
pub mod rollback;
pub mod run;
pub mod send;
pub mod status;
pub mod sync;
pub mod util;
pub mod wait;

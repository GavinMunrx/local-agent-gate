pub mod classifier;
pub mod policy;
pub mod types;

pub use classifier::classify;
pub use policy::PolicyConfig;
pub use types::*;

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new())
}

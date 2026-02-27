pub mod core; // TODO: Swap to private
mod error;

pub mod mx {
    pub use crate::core;
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;
}

pub mod file_lock;
pub mod transaction;
use file_lock::NixFile;
pub use transaction::Transaction;

use crate::{core::transaction::transaction::BuildCommand, mx};

pub fn make_transaction<F, R>(
    description: &str,
    config_dir: &str,
    file_path: &str,
    build_command: BuildCommand,
    f: F,
) -> mx::Result<R>
where
    F: FnOnce(&mut NixFile) -> mx::Result<R>,
{
    let mut transaction = Transaction::new(config_dir, description, build_command)?;
    transaction.add_file(file_path)?;
    transaction.begin()?;

    let file = match transaction.get_file(file_path) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match f(file) {
        Ok(ret) => {
            transaction.commit()?;
            return Ok(ret);
        }
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

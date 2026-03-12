use crate::{
    CONFIG_DIRECTORY,
    core::{
        option::Option as mxOption,
        transaction::{Transaction, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const FILE_MODULE_PATH: &str = "modules.nix";

fn with_module_transaction<F>(description: &str, f: F) -> mx::Result<()>
where
    F: FnOnce(&mut NixFile) -> mx::Result<()>,
{
    let mut transaction = Transaction::new(CONFIG_DIRECTORY, description, BuildCommand::Switch)?;
    transaction.add_file(FILE_MODULE_PATH)?;
    transaction.begin()?;

    let file = match transaction.get_file(FILE_MODULE_PATH) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match f(file) {
        Ok(()) => transaction.commit(),
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

pub fn add_modules(module_path: &str) -> mx::Result<()> {
    with_module_transaction(&format!("Add module {}", module_path), |file| {
        mxOption::new(&format!("modulix.modules.{}.enable", module_path)).set(file, "true")?;
        Ok(())
    })
}

pub fn remove_modules(module_path: &str) -> mx::Result<()> {
    with_module_transaction(&format!("Add module {}", module_path), |file| {
        mxOption::new(&format!("modulix.modules.{}.enable", module_path))
            .set_option_to_default(file)?;
        Ok(())
    })
}

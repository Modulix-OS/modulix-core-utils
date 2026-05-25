pub mod file_lock;
pub mod transaction;

use crate::{
    core::transaction::transaction::{BuildCommand, TransactionPermission, UpdateInput},
    mx,
};
use file_lock::NixFile;
pub use transaction::Transaction;

/// Point d'entrée haut niveau pour effectuer une opération sur un fichier Nix
/// au sein d'une transaction atomique.
///
/// Cette fonction orchestre l'intégralité du cycle de vie d'une transaction :
/// création, ajout du fichier cible, ouverture, exécution de la logique métier
/// fournie par l'appelant, puis commit ou rollback automatique selon le résultat.
///
/// # Comportement
/// 1. Crée une nouvelle [`Transaction`] avec la description et la commande de build fournies.
/// 2. Ajoute `file_path` à la transaction et ouvre une transaction (`begin`).
/// 3. Passe le [`NixFile`] correspondant à la closure `f`.
/// 4. Si `f` retourne `Ok` → [`Transaction::commit`] est appelé.
/// 5. Si `f` retourne `Err`, ou si `get_file` échoue → [`Transaction::rollback`] est appelé.
///
/// # Arguments
/// * `description`     – Libellé humain de la transaction (utilisé pour les logs / historique).
/// * `config_dir`      – Répertoire racine de la configuration NixOS.
/// * `file_path`       – Chemin relatif du fichier Nix à modifier.
/// * `build_command`   – Commande à exécuter après le commit (ex. `nixos-rebuild switch`).
/// * `f`               – Closure recevant le [`NixFile`] ouvert ; doit retourner `mx::Result<R>`.
///
/// # Retour
/// Retourne `Ok(R)` si la transaction s'est terminée avec succès, ou une
/// `mx::ErrorKind` en cas d'échec à n'importe quelle étape.
///
/// # Exemple
/// ```ignore
/// make_transaction(
///     "activer nginx",
///     "/etc/nixos",
///     "/services/nginx.nix",
///     BuildCommand::Switch,
///     |file| {
///         let content = file.get_mut_file_content()?;
///         content.push_str("  services.nginx.enable = true;\n");
///         Ok(())
///     },
/// )?;
/// ```
pub fn make_transaction<F, R>(
    description: &str,
    config_dir: &str,
    file_path: &str,
    build_command: BuildCommand,
    updated_input: UpdateInput,
    f: F,
) -> mx::Result<R>
where
    F: FnOnce(&mut NixFile) -> mx::Result<R>,
{
    let mut transaction = Transaction::new(
        config_dir,
        description,
        build_command,
        TransactionPermission::Writtable,
    )?;
    transaction.add_file(file_path)?;
    transaction.begin()?;

    let file = match transaction.get_file_mut(file_path) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };
    match f(file) {
        Ok(ret) => {
            transaction.commit(updated_input)?;
            Ok(ret)
        }
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

pub fn make_transaction_read_only<F, R>(
    description: &str,
    config_dir: &str,
    file_path: &str,
    build_command: BuildCommand,
    f: F,
) -> mx::Result<R>
where
    F: FnOnce(&NixFile) -> mx::Result<R>,
{
    let mut transaction = Transaction::new(
        config_dir,
        description,
        build_command,
        TransactionPermission::ReadOnly,
    )?;
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
            transaction.commit(UpdateInput::Keep)?;
            Ok(ret)
        }
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

use std::{collections::HashMap, fs, path, process};

use super::file_lock::NixFile;
use crate::{CONFIG_NAME, core::list::List as mxList, mx};

/// Chemin du verrou global empêchant deux builds simultanés.
const LOCK_BUILD_FILE: &str = "/tmp/mx-build.lock";

/// Chemin du verrou de file d'attente : un seul processus peut entrer en zone
/// de build à la fois ; les autres attendent ou passent leur tour.
const LOCK_QUEUE_BUILD_FILE: &str = "/tmp/mx-queue-build.lock";

/// Commande `nixos-rebuild` (ou `nixos-install`) à exécuter après un commit réussi.
///
/// En mode `debug` (sans `--release`), toutes les variantes déclenchent `build-vm`
/// pour éviter de modifier le système hôte pendant le développement.
#[derive(Clone)]
pub enum BuildCommand {
    /// Reconstruit le système et bascule immédiatement (`nixos-rebuild switch`).
    Switch,
    /// Prépare le prochain démarrage sans redémarrer (`nixos-rebuild boot`).
    Boot,
    /// Installation initiale sur une nouvelle machine (`nixos-install`).
    /// La commande de build est vide en release ; déclenche `build-vm` en debug.
    Install,
}

// ─────────────────────────────────────────────────────────────────────────────
// LockFile – verrou de fichier POSIX léger
// ─────────────────────────────────────────────────────────────────────────────

/// Verrou de fichier utilisé pour sérialiser les builds NixOS.
///
/// Le verrou est acquis à la création via [`LockFile::lock`] ou [`LockFile::try_lock`]
/// et libéré explicitement via [`LockFile::unlock`]. Si `unlock` n'est pas appelé,
/// le verrou est libéré par le noyau à la fermeture du processus (mais pas du `File`
/// Rust — préférer `unlock` explicite).
struct LockFile {
    /// Handle vers le fichier verrouillé. `None` après un `unlock`.
    file: Option<fs::File>,
}

impl LockFile {
    /// Crée (ou tronque) le fichier à `path` et pose un verrou exclusif bloquant.
    ///
    /// Bloque jusqu'à l'acquisition du verrou.
    ///
    /// # Erreurs
    /// * `mx::ErrorKind::FailToLock` – Impossible de verrouiller.
    /// * `mx::ErrorKind::IOError`    – Impossible de créer le fichier.
    pub fn lock(path: &str) -> mx::Result<Self> {
        Ok(LockFile {
            file: match fs::File::create(path) {
                Ok(f) => match f.lock() {
                    Ok(_) => Some(f),
                    Err(_) => return Err(mx::ErrorKind::FailToLock),
                },
                Err(e) => return Err(mx::ErrorKind::IOError(e)),
            },
        })
    }

    /// Tente de poser un verrou exclusif non-bloquant.
    ///
    /// # Retour
    /// * `Ok(Some(lock))` – Verrou acquis.
    /// * `Ok(None)`       – Le fichier est déjà verrouillé par un autre processus.
    /// * `Err(_)`         – Erreur I/O inattendue.
    pub fn try_lock(path: &str) -> mx::Result<Option<Self>> {
        Ok(Some(LockFile {
            file: match fs::File::create(path) {
                Ok(f) => match f.try_lock() {
                    Ok(_) => Some(f),
                    Err(fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(_) => return Err(mx::ErrorKind::FailToLock),
                },
                Err(e) => return Err(mx::ErrorKind::IOError(e)),
            },
        }))
    }

    /// Libère le verrou et ferme le handle. Sans effet si déjà déverrouillé.
    pub fn unlock(&mut self) {
        if self.file.is_some() {
            self.file.as_mut().unwrap().unlock().unwrap_or_default();
        }
        self.file = None;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BuildCommand – sélection de la commande de reconstruction
// ─────────────────────────────────────────────────────────────────────────────

impl BuildCommand {
    /// Retourne l'argument passé à `nixos-rebuild` pour cette commande.
    ///
    /// En mode release :
    /// * `Switch`  → `"switch"`
    /// * `Boot`    → `"boot"`
    /// * `Install` → `""` (utilise `nixos-install` directement, cf. [`Transaction::rebuild_config`])
    ///
    /// En mode debug : toutes les variantes retournent `"build-vm"` pour ne pas
    /// modifier le système hôte.
    #[cfg(not(debug_assertions))]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "switch",
            BuildCommand::Boot => "boot",
            BuildCommand::Install => "",
        }
    }

    #[cfg(debug_assertions)]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "build-vm",
            BuildCommand::Boot => "build-vm",
            BuildCommand::Install => "build-vm",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transaction
// ─────────────────────────────────────────────────────────────────────────────

/// Unité de travail atomique sur un dépôt Git de configuration NixOS.
///
/// Une `Transaction` regroupe un ensemble de [`NixFile`] à modifier, exécute les
/// changements en mémoire, puis soit les valide (`commit`) — ce qui déclenche un
/// commit Git et un rebuild NixOS — soit les annule (`rollback`) — ce qui restaure
/// l'état Git précédent et remet les fichiers en place.
///
/// # Cycle de vie
/// ```text
/// Transaction::new(...)
///   └─ add_file(path)   // avant begin
///   └─ begin()          // ouvre le dépôt Git, verrouille les fichiers
///       └─ get_file(path) → &mut NixFile  // modifications en mémoire
///       └─ commit()     // écrit sur disque, commit Git, rebuild
///         ou rollback() // annule tout, restaure l'état précédent
/// ```
///
/// # Invariants
/// * `git_repo.is_some()` ⟺ transaction active (entre `begin` et `commit`/`rollback`).
/// * `old_commit` contient l'OID du commit HEAD au moment du `begin`, permettant
///   un rollback précis même si des fichiers ont été créés.
pub struct Transaction<'a> {
    /// Description humaine de la transaction, utilisée comme message de commit Git.
    info: String,

    /// Table associant chaque chemin relatif à son [`NixFile`] correspondant.
    list_file: HashMap<String, NixFile>,

    /// Chemin absolu vers la racine du dépôt Git de configuration NixOS.
    git_repo_path: String,

    /// Handle vers le dépôt Git, présent uniquement pendant une transaction active.
    git_repo: Option<git2::Repository>,

    /// Identité Git utilisée comme auteur et committeur.
    git_user: git2::Signature<'a>,

    /// Commande de reconstruction à exécuter après le commit.
    build_type: BuildCommand,

    /// OID du commit HEAD capturé au `begin`, utilisé comme point de retour
    /// pour le `rollback`. Vaut `Oid::zero()` si le dépôt était vide.
    old_commit: git2::Oid,

    /// OID du commit de stash créé par [`begin`] si le dépôt contenait des
    /// modifications non commitées. `None` si aucun stash n'a été nécessaire.
    /// Restauré automatiquement par [`commit`] et [`rollback`].
    stash_oid: Option<git2::Oid>,
}

impl<'a> Transaction<'a> {
    /// Crée une nouvelle transaction sans l'ouvrir.
    ///
    /// Aucune opération Git ou I/O n'est effectuée ici.
    ///
    /// # Arguments
    /// * `config_dir`               – Chemin vers le dépôt Git NixOS.
    /// * `transaction_description`  – Message de commit Git.
    /// * `build_type`               – Commande à exécuter après le commit.
    pub fn new(
        config_dir: &str,
        transaction_description: &str,
        build_type: BuildCommand,
    ) -> mx::Result<Self> {
        Ok(Transaction {
            info: transaction_description.to_string(),
            list_file: HashMap::new(),
            git_repo: None,
            git_repo_path: config_dir.to_string(),
            git_user: git2::Signature::now("Modulix-OS", "modulix.os@ik-mail.com").unwrap(),
            build_type,
            old_commit: git2::Oid::zero(),
            stash_oid: None,
        })
    }

    /// Lance la reconstruction NixOS en sous-processus et attend sa fin.
    ///
    /// Selon la variante de `build_command` :
    /// * [`BuildCommand::Install`] → `nixos-install --root /mnt --no-root-password --flake …`
    /// * [`BuildCommand::Switch`] / [`BuildCommand::Boot`] → `nixos-rebuild <cmd> --flake …`
    ///
    /// La sortie standard est héritée (visible dans le terminal parent) ; la sortie
    /// d'erreur est capturée dans `stderr` si fournie.
    ///
    /// # Retour
    /// `Ok(true)` si le processus s'est terminé avec succès (code 0), `Ok(false)` sinon.
    fn rebuild_config(
        path_config: &str,
        config_name: &str,
        build_command: BuildCommand,
        stderr: Option<&mut String>,
    ) -> mx::Result<bool> {
        let mut child = match build_command {
            BuildCommand::Install => process::Command::new("nixos-install")
                .arg("--root")
                .arg("/mnt")
                .arg("--no-root-password")
                .arg("--flake")
                .arg(format!("{}#{}", path_config, config_name))
                .stdout(process::Stdio::inherit())
                .stderr(process::Stdio::piped())
                .spawn()
                .map_err(mx::ErrorKind::IOError)?,
            BuildCommand::Switch | BuildCommand::Boot => process::Command::new("nixos-rebuild")
                .arg(build_command.as_str())
                .arg("--flake")
                .arg(format!("{}#{}", path_config, config_name))
                .stdout(process::Stdio::inherit())
                .stderr(process::Stdio::piped())
                .spawn()
                .map_err(mx::ErrorKind::IOError)?,
        };

        let stderr_output = {
            let mut s = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read;
                err.read_to_string(&mut s).map_err(mx::ErrorKind::IOError)?;
            }
            s
        };
        let status = child.wait().map_err(mx::ErrorKind::IOError)?;
        if let Some(s) = stderr {
            *s = stderr_output;
        }
        Ok(status.success())
    }

    /// Vérifie si `flake.lock` a été modifié (suivi ou non suivi) dans le dépôt Git.
    ///
    /// Utilisé avant chaque commit pour inclure automatiquement les mises à jour
    /// du lockfile Nix dans le commit Git.
    ///
    /// # Erreurs
    /// `mx::ErrorKind::TransactionNotBegin` si la transaction n'est pas active.
    fn flake_lock_modified(&self) -> mx::Result<bool> {
        let repo = self
            .git_repo
            .as_ref()
            .ok_or(mx::ErrorKind::TransactionNotBegin)?;

        let statuses = repo.statuses(None).map_err(mx::ErrorKind::GitError)?;

        Ok(statuses.iter().any(|s| {
            s.path() == Some("flake.lock")
                && s.status().intersects(
                    git2::Status::WT_MODIFIED
                        | git2::Status::WT_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_NEW,
                )
        }))
    }

    /// Retourne `true` si `flake.lock` existe physiquement dans le répertoire du dépôt.
    ///
    /// Si le fichier est absent, un `nix flake update` sera lancé avant le commit
    /// pour générer le lockfile initial.
    fn flake_lock_exists(&self) -> bool {
        path::Path::new(&self.git_repo_path)
            .join("flake.lock")
            .exists()
    }

    /// Crée un commit Git avec l'arbre de travail courant.
    ///
    /// Si `flake.lock` a été modifié, il est automatiquement inclus dans l'index
    /// avant la création du commit.
    ///
    /// Le commit est créé sans parent si le dépôt est vide (premier commit).
    ///
    /// # Arguments
    /// * `update_ref`  – Référence à mettre à jour (ex. `Some("HEAD")`).
    /// * `author`      – Signature de l'auteur.
    /// * `committer`   – Signature du committeur.
    /// * `message`     – Message du commit.
    fn git_commit(
        &self,
        update_ref: Option<&str>,
        author: &git2::Signature<'_>,
        committer: &git2::Signature<'_>,
        message: &str,
    ) -> mx::Result<()> {
        let mut index = self
            .git_repo
            .as_ref()
            .unwrap()
            .index()
            .map_err(mx::ErrorKind::GitError)?;

        // Inclure flake.lock si modifié
        if self.flake_lock_modified()? {
            index
                .add_path(std::path::Path::new("flake.lock"))
                .map_err(mx::ErrorKind::GitError)?;
            index.write().map_err(mx::ErrorKind::GitError)?;
        }

        let tree_oid = index.write_tree().map_err(mx::ErrorKind::GitError)?;
        let tree = self
            .git_repo
            .as_ref()
            .unwrap()
            .find_tree(tree_oid)
            .map_err(mx::ErrorKind::GitError)?;

        // Récupère le commit parent s'il existe (None pour le premier commit)
        let parent = self
            .git_repo
            .as_ref()
            .unwrap()
            .head()
            .and_then(|h| h.peel_to_commit())
            .ok();

        let parents: Vec<&git2::Commit> = parent.iter().collect();

        self.git_repo
            .as_ref()
            .unwrap()
            .commit(update_ref, author, committer, message, &tree, &parents)
            .map_err(mx::ErrorKind::GitError)?;
        Ok(())
    }

    /// Détermine si un fichier a été modifié depuis le commit `oid`.
    ///
    /// Utilisé dans [`commit_impl`] pour n'inclure dans le commit Git que les
    /// fichiers effectivement modifiés, évitant des commits vides.
    ///
    /// Si `oid` est zéro (dépôt vide), le fichier est toujours considéré comme nouveau.
    fn has_diff_with_commit(
        repo: &git2::Repository,
        oid: git2::Oid,
        file_path: &str,
    ) -> mx::Result<bool> {
        if oid.is_zero() {
            return Ok(true);
        }
        let commit = repo.find_commit(oid).unwrap();
        let commit_tree = commit.tree().unwrap();

        let status = repo
            .status_file(path::Path::new(file_path))
            .map_err(mx::ErrorKind::GitError)?;

        // Fichier nouveau : forcément différent
        if status.contains(git2::Status::WT_NEW) || status.contains(git2::Status::INDEX_NEW) {
            return Ok(true);
        }

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.pathspec(path::Path::new(file_path));
        let diff = repo
            .diff_tree_to_workdir_with_index(Some(&commit_tree), Some(&mut diff_opts))
            .unwrap();
        Ok(diff.stats().unwrap().files_changed() > 0)
    }

    /// Ajoute un fichier à l'index Git (équivalent de `git add <path>`).
    fn git_add(&self, path: &str) -> Result<(), mx::ErrorKind> {
        let repo = self.git_repo.as_ref().unwrap();
        let mut index = repo.index().map_err(mx::ErrorKind::GitError)?;
        index
            .add_path(path::Path::new(path))
            .map_err(mx::ErrorKind::GitError)?;
        index.write().map_err(mx::ErrorKind::GitError)?;
        Ok(())
    }

    /// Enregistre un fichier Nix à inclure dans la transaction.
    ///
    /// Doit être appelé **avant** [`begin`]. Appeler cette méthode après `begin`
    /// retourne `mx::ErrorKind::TransactionAlreadyBegin`.
    ///
    /// `configuration.nix` est automatiquement ajouté par [`begin`] ; il n'est
    /// pas nécessaire de l'ajouter manuellement.
    ///
    /// # Arguments
    /// * `path` – Chemin relatif à la racine du dépôt (ex. `"/services/nginx.nix"`).
    pub fn add_file(&mut self, path: &str) -> mx::Result<()> {
        if self.git_repo.is_some() {
            return Err(mx::ErrorKind::TransactionAlreadyBegin);
        }
        self.list_file
            .insert(path.to_string(), NixFile::new(&self.git_repo_path, path));
        Ok(())
    }

    /// Indique si une transaction est actuellement active.
    #[allow(dead_code)]
    pub fn as_begin(&self) -> bool {
        self.git_repo.is_some()
    }

    /// Retourne une référence mutable vers le [`NixFile`] associé à `path`.
    ///
    /// # Erreurs
    /// * `mx::ErrorKind::TransactionNotBegin` – `begin` n'a pas encore été appelé.
    /// * `mx::ErrorKind::FileNotFound`        – `path` n'a pas été ajouté via `add_file`.
    pub fn get_file(&mut self, path: &str) -> mx::Result<&mut NixFile> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        self.list_file
            .get_mut(path)
            .ok_or(mx::ErrorKind::FileNotFound)
    }

    /// Ouvre la transaction : initialise le dépôt Git, stashe les éventuelles
    /// modifications non commitées, verrouille et charge tous les fichiers enregistrés.
    ///
    /// Étapes effectuées :
    /// 1. Ajoute automatiquement `configuration.nix` aux fichiers suivis.
    /// 2. Ouvre le dépôt Git à `git_repo_path`.
    /// 3. Si le dépôt contient des modifications non commitées, elles sont stashées
    ///    avec `INCLUDE_UNTRACKED` et restaurées automatiquement en fin de transaction.
    /// 4. Appelle [`NixFile::begin`] sur chaque fichier ; crée les fichiers absents
    ///    et les ajoute à la liste `imports` de `configuration.nix`.
    /// 5. Capture l'OID du commit HEAD courant pour un éventuel rollback.
    ///
    /// # Erreurs
    /// * `mx::ErrorKind::GitError`              – Dépôt introuvable ou erreur Git.
    /// * `mx::ErrorKind::TransactionAlreadyBegin` – `begin` déjà appelé.
    pub fn begin(&mut self) -> mx::Result<()> {
        self.add_file("configuration.nix")?;
        let mut new_file: Vec<String> = vec![];
        {
            self.git_repo =
                Some(git2::Repository::open(&self.git_repo_path).map_err(mx::ErrorKind::GitError)?);

            let is_empty = self
                .git_repo
                .as_ref()
                .unwrap()
                .is_empty()
                .map_err(mx::ErrorKind::GitError)?;

            // Si le dépôt contient des modifications non commitées, on les stashe
            // pour travailler sur un arbre propre et les restaurer après.
            if !is_empty {
                let is_dirty = {
                    let mut opts = git2::StatusOptions::new();
                    opts.include_untracked(true).include_ignored(false);
                    let statuses = self
                        .git_repo
                        .as_ref()
                        .unwrap()
                        .statuses(Some(&mut opts))
                        .map_err(mx::ErrorKind::GitError)?;
                    !statuses.is_empty()
                }; // `statuses` est droppé ici, libérant l'emprunt immutable

                if is_dirty {
                    let stash_oid = self
                        .git_repo
                        .as_mut()
                        .unwrap()
                        .stash_save(
                            &self.git_user,
                            "mx: auto-stash before transaction",
                            Some(git2::StashFlags::INCLUDE_UNTRACKED),
                        )
                        .map_err(mx::ErrorKind::GitError)?;
                    self.stash_oid = Some(stash_oid);
                }
            }

            for (path_file, file) in self.list_file.iter_mut() {
                match file.begin() {
                    Ok(_) => (),
                    Err(mx::ErrorKind::FileNotFound) => {
                        // Le fichier n'existe pas encore : on le crée et on note
                        // qu'il devra être déclaré dans configuration.nix
                        file.create_file()?;
                        file.begin()?;
                        new_file.push(path_file.clone());
                    }
                    Err(e) => return Err(e),
                }
            }

            // Capture du commit courant pour le rollback
            self.old_commit = match self.git_repo.as_ref().unwrap().head() {
                Ok(head) => head.peel_to_commit().map_err(mx::ErrorKind::GitError)?.id(),
                Err(e)
                    if e.code() == git2::ErrorCode::UnbornBranch
                        || e.code() == git2::ErrorCode::NotFound =>
                {
                    git2::Oid::zero()
                }
                Err(e) => return Err(mx::ErrorKind::GitError(e)),
            };
        }
        {
            // Ajoute les nouveaux fichiers à la liste imports de configuration.nix
            let config_file = self.get_file("configuration.nix")?;
            let import_file = mxList::new("imports", true);
            for path in new_file {
                import_file.add(config_file, &format!("./{}", &path))?;
            }
        }
        Ok(())
    }

    /// Restaure le stash créé par [`begin`] s'il en existe un.
    ///
    /// Appelé en fin de [`commit_impl`] et de [`rollback`] pour remettre en place
    /// les modifications qui étaient présentes avant l'ouverture de la transaction.
    ///
    /// En cas d'échec du `stash_pop` (conflit), l'erreur est propagée mais
    /// `stash_oid` est quand même réinitialisé pour éviter une double tentative.
    fn stash_restore(&mut self) -> mx::Result<()> {
        if self.stash_oid.take().is_some() {
            self.git_repo
                .as_mut()
                .unwrap()
                .stash_pop(0, None)
                .map_err(mx::ErrorKind::GitError)?;
        }
        Ok(())
    }

    /// Implémentation interne du commit, séparée pour permettre au wrapper
    /// [`commit`] de déclencher un rollback automatique en cas d'échec.
    ///
    /// Étapes :
    /// 1. Commit de chaque [`NixFile`] sur disque.
    /// 2. Détection des fichiers réellement modifiés (`git add` sélectif).
    /// 3. Si au moins un fichier a changé :
    ///    a. Génère `flake.lock` si absent (`nix flake update`).
    ///    b. Crée le commit Git.
    ///    c. Tente d'acquérir le verrou de build ; si obtenu, lance `nixos-rebuild`.
    /// 4. Ferme tous les [`NixFile`] et libère le dépôt Git.
    fn commit_impl(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.commit()?;
        }

        let mut need_modif = false;
        for (path, _) in self.list_file.iter() {
            if Self::has_diff_with_commit(self.git_repo.as_ref().unwrap(), self.old_commit, path)? {
                need_modif = true;
                self.git_add(path)?;
            }
        }

        if need_modif {
            // Génère flake.lock s'il n'existe pas encore
            if !self.flake_lock_exists() {
                process::Command::new("nix")
                    .args(["flake", "update"])
                    .current_dir(&self.git_repo_path)
                    .output()
                    .map_err(mx::ErrorKind::IOError)?;
            }
            self.git_commit(Some("HEAD"), &self.git_user, &self.git_user, &self.info)?;

            // Sérialisation du build : on n'entre dans la zone critique que si
            // personne d'autre n'attend déjà (try_lock sur la file d'attente)
            let mut queue = LockFile::try_lock(LOCK_QUEUE_BUILD_FILE)?;
            if queue.is_some() {
                let mut lock_build = LockFile::lock(LOCK_BUILD_FILE)?;
                queue.as_mut().unwrap().unlock();
                let mut stderr = String::new();
                let success = Self::rebuild_config(
                    &self.git_repo_path,
                    CONFIG_NAME,
                    self.build_type.clone(),
                    Some(&mut stderr),
                )?;
                lock_build.unlock();
                if !success {
                    return Err(mx::ErrorKind::BuildError(stderr));
                }
            }
        }

        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.close()?;
        }
        // Restaure les modifications stashées avant la transaction
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
    /// persiste les modifications, crée un commit Git
    /// et déclenche la reconstruction NixOS.
    ///
    /// En cas d'échec interne, un [`rollback`] automatique est tenté avant de
    /// propager l'erreur.
    pub fn commit(&mut self) -> mx::Result<()> {
        self.commit_impl().map_err(|e| {
            let _ = self.rollback();
            e
        })
    }

    /// Annule la transaction et restaure l'état précédent du dépôt Git.
    ///
    /// Étapes :
    /// 1. Si le dépôt était vide au `begin` (`old_commit` zéro) : ferme les fichiers
    ///    et sort sans toucher à Git.
    /// 2. Sinon : repointe la branche courante sur `old_commit` et effectue un
    ///    `checkout --force` pour restaurer l'arbre de travail.
    /// 3. Supprime les fichiers créés pendant la transaction ; remet le flag
    ///    immutable sur les fichiers préexistants restaurés.
    ///
    /// # Erreurs
    /// `mx::ErrorKind::TransactionNotBegin` si aucune transaction n'est active.
    pub fn rollback(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }

        {
            // Cas particulier : dépôt vide, aucun commit à restaurer
            if self.old_commit.is_zero() {
                for (_, nix_file) in self.list_file.iter_mut() {
                    let _ = nix_file.close();
                }
                self.git_repo = None;
                return Ok(());
            }

            let repo = self.git_repo.as_ref().unwrap();
            let head = repo.head().map_err(mx::ErrorKind::GitError)?;

            let refname = head
                .name()
                .ok_or(mx::ErrorKind::GitError(git2::Error::from_str(
                    "HEAD is not a symbolic ref",
                )))?;

            // Repointe la référence HEAD sur l'ancien commit
            repo.find_reference(refname)
                .map_err(mx::ErrorKind::GitError)?
                .set_target(self.old_commit, "reset to previous commit")
                .map_err(mx::ErrorKind::GitError)?;

            repo.set_head(refname).map_err(mx::ErrorKind::GitError)?;

            // Rend les fichiers mutables pour que checkout puisse les écraser
            for (_, nix_file) in self.list_file.iter_mut() {
                NixFile::make_mutable(nix_file.get_file_path()).ok();
            }

            // Force la restauration de l'arbre de travail
            let mut checkout = git2::build::CheckoutBuilder::new();
            checkout.force();
            repo.checkout_head(Some(&mut checkout))
                .map_err(mx::ErrorKind::GitError)?;

            // Nettoyage post-checkout :
            // - Fichiers créés pendant la transaction → suppression
            // - Fichiers préexistants → remise du flag immutable
            for (_, nix_file) in self.list_file.iter_mut() {
                if nix_file.was_created() {
                    NixFile::make_mutable(nix_file.get_file_path()).ok();
                    std::fs::remove_file(nix_file.get_file_path()).ok();
                } else if path::Path::new(nix_file.get_file_path()).exists() {
                    NixFile::make_immutable(nix_file.get_file_path()).ok();
                }
            }
        }
        // Restaure les modifications stashées avant la transaction
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
}

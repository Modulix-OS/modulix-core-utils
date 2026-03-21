use crate::mx;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
};

use nix::libc;
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

/// Représente un fichier NixOS géré avec des garanties d'intégrité via les attributs
/// étendus du système de fichiers ext2/ext4 (flag `immutable`).
///
/// Un `NixFile` encapsule l'accès à un fichier de configuration Nix en suivant
/// un cycle de vie explicite : `begin` → modifications → `commit` ou `close`.
/// Tant qu'une transaction n'est pas ouverte via `begin`, la lecture/écriture
/// du contenu est interdite.
///
/// Le flag `immutable` du noyau Linux est utilisé pour protéger le fichier entre
/// deux transactions. Il n'est retiré que le temps d'une transaction active, puis
/// restauré au `commit`.
pub struct NixFile {
    /// Handle vers le fichier ouvert, présent uniquement pendant une transaction active.
    file: Option<fs::File>,

    /// Chemin absolu vers le fichier sur le système de fichiers.
    path: String,

    /// Contenu textuel du fichier, chargé en mémoire lors du `begin`.
    file_content: String,

    /// Indique si le fichier a été créé par `create_file` (absent au départ).
    was_created: bool,
}

impl NixFile {
    /// Construit un nouveau `NixFile` à partir d'un chemin de dépôt et d'un chemin relatif.
    ///
    /// Le fichier n'est pas ouvert à ce stade ; aucune opération I/O n'est effectuée.
    ///
    /// # Arguments
    /// * `repo_path` – Chemin racine du dépôt NixOS (ex. `/etc/nixos`).
    /// * `relative_path` – Chemin du fichier relatif à `repo_path` (ex. `/hardware.nix`).
    pub fn new(repo_path: &str, relative_path: &str) -> Self {
        NixFile {
            file: None,
            path: String::from(repo_path) + relative_path,
            file_content: String::new(),
            was_created: false,
        }
    }

    /// Flag ext2/ext4 indiquant qu'un fichier est immuable (lecture seule au niveau noyau).
    /// Valeur issue de `<linux/fs.h>` : `FS_IMMUTABLE_FL`.
    const EXT2_IMMUTABLE_FL: libc::c_long = 0x00000010;

    /// Numéro ioctl pour lire les flags d'un fichier (`FS_IOC_GETFLAGS`).
    const FS_IOC_GETFLAGS: libc::c_ulong = 0x80086601;

    /// Numéro ioctl pour écrire les flags d'un fichier (`FS_IOC_SETFLAGS`).
    const FS_IOC_SETFLAGS: libc::c_ulong = 0x40086602;

    /// Vérifie si le fichier situé à `path` appartient à l'utilisateur root (uid 0).
    ///
    /// Les opérations `ioctl` sur les flags immutables ne sont significatives que pour
    /// les fichiers root ; cette vérification évite des erreurs silencieuses sur des
    /// fichiers appartenant à d'autres utilisateurs.
    fn is_owned_by_root(path: &str) -> mx::Result<bool> {
        let metadata = std::fs::metadata(path).map_err(mx::ErrorKind::IOError)?;
        Ok(metadata.uid() == 0)
    }

    /// Lit les flags ioctl courants du fichier situé à `path`.
    ///
    /// Ouvre le fichier en lecture seule et exécute `FS_IOC_GETFLAGS` via `ioctl`.
    ///
    /// # Erreurs
    /// Retourne `mx::ErrorKind::UnixError` si l'appel `ioctl` échoue.
    fn get_flags(path: &str) -> mx::Result<libc::c_long> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(mx::ErrorKind::IOError)?;
        let fd = file.as_raw_fd();
        let mut flags: libc::c_long = 0;

        unsafe {
            if libc::ioctl(fd, Self::FS_IOC_GETFLAGS, &mut flags) < 0 {
                return Err(mx::ErrorKind::UnixError(nix::Error::last()));
            }
        }
        Ok(flags)
    }

    /// Active le flag `immutable` sur le fichier situé à `path`.
    ///
    /// Cette opération n'est effectuée que si le fichier appartient à root.
    /// Une fois immutable, le fichier ne peut plus être modifié ni supprimé,
    /// même par root, sans retirer explicitement le flag.
    ///
    /// Appelé automatiquement après `create_file` et après `commit`.
    ///
    /// # Erreurs
    /// Retourne `mx::ErrorKind::UnixError` si l'appel `ioctl` échoue.
    pub(super) fn make_immutable(path: &str) -> mx::Result<()> {
        if Self::is_owned_by_root(path)? {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(mx::ErrorKind::IOError)?;
            let fd = file.as_raw_fd();
            let mut flags = Self::get_flags(path)?;

            // Active le bit immutable dans les flags
            flags |= Self::EXT2_IMMUTABLE_FL;

            unsafe {
                if libc::ioctl(fd, Self::FS_IOC_SETFLAGS, &flags) < 0 {
                    return Err(mx::ErrorKind::UnixError(nix::Error::last()));
                }
            }
        }
        Ok(())
    }

    /// Désactive le flag `immutable` sur le fichier situé à `path`.
    ///
    /// Cette opération n'est effectuée que si le fichier appartient à root.
    /// Doit être appelée avant toute écriture sur un fichier précédemment rendu immutable.
    ///
    /// Appelé automatiquement au début de `begin`.
    ///
    /// # Erreurs
    /// Retourne `mx::ErrorKind::UnixError` si l'appel `ioctl` échoue.
    pub(super) fn make_mutable(path: &str) -> mx::Result<()> {
        if Self::is_owned_by_root(path)? {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(mx::ErrorKind::IOError)?;
            let fd = file.as_raw_fd();
            let mut flags = Self::get_flags(path)?;

            // Désactive le bit immutable dans les flags
            flags &= !Self::EXT2_IMMUTABLE_FL;

            unsafe {
                if libc::ioctl(fd, Self::FS_IOC_SETFLAGS, &flags) < 0 {
                    return Err(mx::ErrorKind::UnixError(nix::Error::last()));
                }
            }
        }
        Ok(())
    }

    /// Crée physiquement le fichier Nix sur le disque avec un squelette de module vide.
    ///
    /// Le contenu initial est `{config, lib, pkgs, ...}:\n{\n}\n`, ce qui correspond
    /// à un module NixOS minimal valide.
    ///
    /// Après création, le fichier est rendu immutable pour empêcher toute modification
    /// accidentelle hors transaction.
    ///
    /// # Erreurs
    /// Retourne une erreur I/O si la création ou l'écriture initiale échoue.
    pub(super) fn create_file(&mut self) -> mx::Result<()> {
        let mut file = fs::File::create(&self.path).map_err(mx::ErrorKind::IOError)?;
        file.write_all("{config, lib, pkgs, ...}:\n{\n}\n".as_bytes())
            .map_err(mx::ErrorKind::IOError)?;
        self.was_created = true;
        Self::make_immutable(&self.path)?;
        Ok(())
    }

    /// Indique si le fichier a été créé par cet objet (via `create_file`).
    ///
    /// Utile pour distinguer un fichier nouvellement généré d'un fichier préexistant.
    pub fn was_created(&self) -> bool {
        self.was_created
    }

    /// Retourne le chemin absolu du fichier.
    pub fn get_file_path(&self) -> &str {
        return &self.path;
    }

    /// Retourne une référence mutable sur le contenu du fichier en mémoire.
    ///
    /// # Erreurs
    /// Retourne `mx::ErrorKind::TransactionNotBegin` si aucune transaction n'est active
    /// (c'est-à-dire si `begin` n'a pas encore été appelé avec succès).
    pub fn get_mut_file_content(&mut self) -> mx::Result<&mut String> {
        if self.file.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        Ok(&mut self.file_content)
    }

    /// Retourne une référence partagée sur le contenu du fichier en mémoire.
    ///
    /// # Erreurs
    /// Retourne `mx::ErrorKind::TransactionNotBegin` si aucune transaction n'est active.
    pub fn get_file_content(&self) -> mx::Result<&String> {
        if self.file.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        Ok(&self.file_content)
    }

    /// Ouvre une transaction sur le fichier : retire le flag immutable, pose un verrou
    /// exclusif et charge le contenu en mémoire dans `file_content`.
    ///
    /// Si une transaction est déjà active (`self.file.is_some()`), l'appel est sans effet.
    ///
    /// # Cycle de vie attendu
    /// `begin` → modifications via `get_mut_file_content` → `commit` ou `close`
    ///
    /// # Erreurs
    /// * `mx::ErrorKind::FileNotFound` – Le fichier n'existe pas.
    /// * `mx::ErrorKind::PermissionDenied` – Permissions insuffisantes pour ouvrir le fichier.
    /// * `mx::ErrorKind::FailToLock` – Impossible d'acquérir le verrou de fichier.
    /// * `mx::ErrorKind::IOError` – Autre erreur I/O lors de la lecture.
    pub(super) fn begin(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
            // Rendre le fichier mutable avant toute ouverture en écriture
            match Self::make_mutable(&self.path) {
                Ok(()) => (),
                Err(e) => match e {
                    mx::ErrorKind::IOError(ioe) => match ioe.kind() {
                        // Le fichier n'existe pas encore : on propage une erreur spécifique
                        io::ErrorKind::NotFound => return Err(mx::ErrorKind::FileNotFound),
                        _ => return Err(mx::ErrorKind::IOError(ioe)),
                    },
                    err => return Err(err),
                },
            };

            // Ouvre le fichier existant en lecture+écriture, sans le créer
            self.file = Some(
                File::options()
                    .create(false)
                    .read(true)
                    .write(true)
                    .open(&self.path)
                    .map_err(|e| match e.kind() {
                        io::ErrorKind::PermissionDenied => mx::ErrorKind::PermissionDenied,
                        io::ErrorKind::NotFound => mx::ErrorKind::FileNotFound,
                        _ => mx::ErrorKind::IOError(e),
                    })?,
            )
        }

        // Pose un verrou exclusif puis lit le contenu intégral en mémoire
        if let Some(f) = self.file.as_mut() {
            f.lock().or(Err(mx::ErrorKind::FailToLock))?;
            f.read_to_string(&mut self.file_content)
                .map_err(mx::ErrorKind::IOError)?;
            Ok(())
        } else {
            Err(mx::ErrorKind::InvalidFile)
        }
    }

    /// Valide la transaction : réécrit le contenu en mémoire dans le fichier, remet
    /// le flag immutable et libère le verrou.
    ///
    /// Le fichier est tronqué à zéro avant réécriture pour éviter tout résidu si le
    /// nouveau contenu est plus court que l'ancien.
    ///
    /// # Erreurs
    /// * `mx::ErrorKind::InvalidFile` – Aucune transaction active.
    /// * `mx::ErrorKind::PermissionDenied` – Échec de l'écriture.
    pub(super) fn commit(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
            return Err(mx::ErrorKind::InvalidFile);
        }

        // Retour au début du fichier, puis troncature pour repartir de zéro
        self.file
            .as_mut()
            .unwrap()
            .seek(io::SeekFrom::Start(0))
            .unwrap();
        self.file.as_ref().unwrap().set_len(0).unwrap();

        // Écriture du contenu modifié
        self.file
            .as_ref()
            .unwrap()
            .write_all(&self.file_content.as_bytes())
            .or(Err(mx::ErrorKind::PermissionDenied))?;

        // Protection du fichier et libération du verrou
        Self::make_immutable(&self.path)?;
        self.file
            .as_ref()
            .unwrap()
            .unlock()
            .map_err(mx::ErrorKind::IOError)?;

        // Réinitialise l'état : la transaction est terminée après un commit.
        // Sans ceci, file.is_some() resterait vrai et get_file_content()
        // continuerait de retourner Ok au lieu de TransactionNotBegin.
        self.file_content = String::new();
        self.file = None;
        Ok(())
    }

    /// Annule la transaction sans persister les modifications : libère le verrou,
    /// vide le contenu en mémoire et ferme le handle.
    ///
    /// Le flag immutable n'est PAS restauré ici : si `begin` avait retiré le flag
    /// (fichier root), celui-ci reste mutable après `close`. Préférer `commit` pour
    /// toujours laisser le fichier dans un état protégé.
    ///
    /// # Erreurs
    /// Toujours `Ok(())` (l'erreur de déverrouillage est intentionnellement ignorée).
    pub(super) fn close(&mut self) -> mx::Result<()> {
        if let Some(f) = self.file.as_ref() {
            #[allow(unused_must_use)]
            f.unlock();
        }
        self.file_content = String::new();
        self.file = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "file_lock_tests.rs"]
mod tests;

use std::{ops::Range};
use rnix::{self, TextRange, TextSize};

use crate::mx;

#[derive(Debug, Clone)]
pub struct NewInsertion {
    pos: usize,
    rest_option_path: String,
    indent_level: usize,
}

#[derive(Debug, Clone)]
pub struct ExistingOption {
    range_path: Range<usize>,
    range_value: Range<usize>,
    indent_level: usize,
}

#[derive(Debug, Clone)]
pub enum SettingsPosition {
    NewInsertion(NewInsertion),
    ExistingOption(ExistingOption)
}

impl NewInsertion {
    pub fn new(
        pos: usize,
        rest_option_path: impl Into<String>,
        indent_level: usize,
    ) -> Self {
        NewInsertion {
            pos,
            rest_option_path: rest_option_path.into(),
            indent_level
        }
    }

    pub fn get_pos_new_insertion(&self) -> usize {
        self.pos
    }

    pub fn get_remaining_path(&self) -> &str {
        &self.rest_option_path
    }

    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}

impl ExistingOption {
    pub fn new(
        range_path: Range<usize>,
        range_value: Range<usize>,
        indent_level: usize,
    ) -> Self {
        ExistingOption { range_path, range_value, indent_level }
    }

    pub fn get_range_option(&self) -> &Range<usize> {
        &self.range_path
    }

    pub fn get_range_option_value(&self) -> &Range<usize> {
        &self.range_value
    }

    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}


impl SettingsPosition {
    pub fn get_indent_level(&self) -> usize {
        match &self {
            Self::ExistingOption(ExistingOption { indent_level, .. }) => *indent_level,
            Self::NewInsertion(NewInsertion { indent_level, .. }) => *indent_level,
        }
    }

    pub fn new(nix_ast: &rnix::SyntaxNode, settings: &str) -> mx::Result<Self> {
        Self::localise_option(&nix_ast, &settings, 0usize).ok_or(mx::ErrorType::InvalidFile)
    }


    /// Localise récursivement une option dans l'AST Nix.
    ///
    /// Cette fonction privée est le point d'entrée de l'algorithme de recherche.
    /// Elle dispatch vers les fonctions spécialisées selon le type de nœud rencontré :
    ///
    /// - `NODE_ATTR_SET` : Ensemble d'attributs (`{ ... }`)
    /// - `NODE_ATTRPATH_VALUE` : Attribution (`key = value;`)
    /// - Autres : Parcours récursif des enfants
    ///
    /// # Arguments
    ///
    /// * `ast` - Nœud de l'arbre syntaxique à analyser
    /// * `settings` - Chemin de l'option recherchée
    ///
    /// # Algorithme
    ///
    /// 1. Identifie le type de nœud
    /// 2. Délègue au gestionnaire approprié
    /// 3. Pour les autres nœuds, parcourt récursivement les enfants
    /// 4. Retourne le premier match trouvé
    fn localise_option(
        ast: &rnix::SyntaxNode,
        settings: &str,
        indent_level: usize)
    -> Option<SettingsPosition> {
        return match ast.kind() {
            rnix::SyntaxKind::NODE_ATTR_SET =>
                Some(Self::localise_option_node_attr_set(&ast, &settings, indent_level+1usize)),
            rnix::SyntaxKind::NODE_ATTRPATH_VALUE =>
                Self::localise_option_node_attrpath_value(&ast, &settings, indent_level),
            _ => {
                for c in ast.children() {
                    if let Some(ret) = Self::localise_option(&c, settings, indent_level) {
                        return Some(ret);
                    }
                }
                None
            }
        };
    }

    /// Traite un nœud de type ensemble d'attributs (`NODE_ATTR_SET`).
    ///
    /// Cette fonction recherche la meilleure correspondance parmi tous les enfants
    /// de l'ensemble d'attributs. Elle implémente une stratégie de recherche qui :
    ///
    /// 1. Parcourt tous les enfants à la recherche de correspondances
    /// 2. Conserve le match avec le chemin le plus long (correspondance la plus spécifique)
    /// 3. Retourne immédiatement si un match exact est trouvé (`option_path = None`)
    /// 4. Sinon, retourne le meilleur match partiel ou un point d'insertion
    ///
    /// # Arguments
    ///
    /// * `ast` - Nœud de type `NODE_ATTR_SET` à analyser
    /// * `setting` - Chemin de l'option recherchée
    ///
    /// # Retour
    ///
    /// Toujours retourne une `SettingsPosition` avec trois cas possibles :
    ///
    /// 1. **Match complet** (`option_path = None`) : L'option existe exactement
    /// 2. **Match partiel** (`option_path = Some(...)`) : Une partie du chemin existe
    /// 3. **Aucun match** : Retourne une position d'insertion avant le `}` fermant
    ///
    /// # Exemples
    ///
    /// ```text
    /// // Cas 1: Match complet
    /// // Fichier: { services.nginx.enable = true; }
    /// // Recherche: "services.nginx.enable"
    /// // Résultat: option_path = None
    ///
    /// // Cas 2: Match partiel
    /// // Fichier: { services.nginx = {}; }
    /// // Recherche: "services.nginx.enable"
    /// // Résultat: option_path = Some("enable")
    ///
    /// // Cas 3: Aucun match
    /// // Fichier: { services = {}; }
    /// // Recherche: "network.proxy"
    /// // Résultat: Point d'insertion avant le '}'
    /// ```
    fn localise_option_node_attr_set(
        ast: &rnix::SyntaxNode,
        settings: &str,
        indent_level: usize)
    -> SettingsPosition {
        let mut best_opt_pos: Option<NewInsertion> = None;

        // Parcourir tous les enfants pour trouver des correspondances
        for c in ast.children() {
            let opt_pos = Self::localise_option(&c, &settings, indent_level);
            if let Some(pos) = opt_pos {

                // Si match exact trouvé, retourner immédiatement
                match pos {
                    Self::ExistingOption(p) => return Self::ExistingOption(p),
                    Self::NewInsertion(new_pos) => {
                        match &best_opt_pos {
                            None => best_opt_pos = Some(new_pos),
                            Some(best_pos) =>  {
                                if new_pos.get_remaining_path().len() < best_pos.get_remaining_path().len() {
                                    best_opt_pos = Some(new_pos);
                                }
                            }
                        }
                    }
                }

                // Sinon, conserver le meilleur match (option dans la définition la plus proche)

            }
        }

        // Retourner le meilleur match ou un point d'insertion
        match best_opt_pos {
            Some(best_pos) => SettingsPosition::NewInsertion(best_pos),
            None => SettingsPosition::NewInsertion(NewInsertion::new(
                <TextSize as Into<usize>>::into(ast.text_range().end()) - 1,
                settings,
                indent_level,
            )),
        }
    }

    /// Traite un nœud d'attribution (`NODE_ATTRPATH_VALUE`).
    ///
    /// Cette fonction analyse les nœuds d'attribution (ex: `services.nginx.enable = true;`)
    /// en vérifiant si le chemin d'attribut correspond au setting recherché.
    ///
    /// # Arguments
    ///
    /// * `ast` - Nœud de type `NODE_ATTRPATH_VALUE` à analyser
    /// * `settings` - Chemin complet de l'option recherchée
    ///
    /// # Algorithme
    ///
    /// 1. **Extraction du chemin** : Récupère le chemin d'attribut du nœud
    /// 2. **Vérification du préfixe** : Compare segment par segment avec le setting
    ///    - Compte les segments de chaque chemin (séparés par '.')
    ///    - Vérifie que l'attr_path est un préfixe du setting
    ///    - Compare chaque segment individuellement
    /// 3. **Analyse de la valeur** :
    ///    - Si `NODE_ATTR_SET` : Recherche récursive dans le sous-ensemble
    ///    - Si valeur simple : Retourne la position (match exact)
    ///
    /// # Retour
    ///
    /// - `Some(SettingsPosition)` : Si le chemin d'attribut est un préfixe du setting
    /// - `None` : Si aucune correspondance de préfixe n'est trouvée
    ///
    /// # Correspondance de préfixe
    ///
    /// Un attr_path est considéré comme un préfixe valide si :
    /// - Il a le même nombre ou moins de segments que le setting
    /// - Tous ses segments correspondent aux segments correspondants du setting
    ///
    /// Exemples de préfixes valides :
    ///
    /// ```text
    /// Attr path: services.nginx
    /// Settings:  services.nginx.enable
    /// ✓ Préfixe valide (2 ≤ 3 segments, tous identiques)
    ///
    /// Attr path: services.nginx.enable
    /// Settings:  services.nginx.enable
    /// ✓ Match exact (3 = 3 segments)
    ///
    /// Attr path: services.apache
    /// Settings:  services.nginx.enable
    /// ✗ Pas un préfixe (apache ≠ nginx)
    /// ```
    ///
    /// # Types de valeurs supportés
    ///
    /// - `NODE_ATTR_SET` : Ensemble imbriqué (`{ ... }`)
    /// - `NODE_IDENT` : Identifiant (`true`, `false`, variable)
    /// - `NODE_LITERAL` : Valeur littérale (nombre, boolean)
    /// - `NODE_STRING` : Chaîne de caractères
    /// - `NODE_PATH_REL` : Chemin relatif (`./path`)
    /// - `NODE_PATH_ABS` : Chemin absolu (`/path`)
    /// - `NODE_PATH_HOME` : Chemin home (`~/path`)
    /// - `NODE_PATH_SEARCH` : Chemin de recherche (`<nixpkgs>`)
    fn localise_option_node_attrpath_value(
        ast: &rnix::SyntaxNode,
        settings: &str,
        indent_level: usize)
    -> Option<SettingsPosition> {
        let mut attr_path_valid: Option<String> = None;

        // Étape 1: Trouver le chemin d'attribut qui correspond
        for c in ast.children()
            .filter(|c| c.kind() == rnix::SyntaxKind::NODE_ATTRPATH) {
            let attr_path = c.text().to_string();

            let count_split_settings = settings.split('.').count();
            let count_split_attr_path = attr_path.split('.').count();

            // Vérifier si attr_path est un préfixe de settings
            let is_prefix = count_split_attr_path <=count_split_settings
                && attr_path.split('.').zip(settings.split('.')).all(|(a, s)| a == s);

            if is_prefix  {
                attr_path_valid = Some(attr_path);
                break;
            }
        };

        // Si aucun préfixe valide trouvé, retourner None
        if let None = attr_path_valid {
            return None;
        }

        // Étape 2: Analyser la valeur associée
        let children_value = ast.children()
            .filter(|cv| match cv.kind() {
                rnix::SyntaxKind::NODE_ATTR_SET
                | rnix::SyntaxKind::NODE_LIST
                | rnix::SyntaxKind::NODE_WITH
                | rnix::SyntaxKind::NODE_IDENT
                | rnix::SyntaxKind::NODE_PATH_REL
                | rnix::SyntaxKind::NODE_PATH_ABS
                | rnix::SyntaxKind::NODE_PATH_HOME
                | rnix::SyntaxKind::NODE_PATH_SEARCH
                | rnix::SyntaxKind::NODE_STRING
                | rnix::SyntaxKind::NODE_LITERAL => true,
                _ => false,
            });

        for c in children_value {
            if c.kind() == rnix::SyntaxKind::NODE_ATTR_SET {
                // Cas 1: La valeur est un ensemble imbriqué
                // Retirer le préfixe déjà traité et continuer la recherche
                let setting_whitout_path = settings
                    .strip_prefix(&attr_path_valid.unwrap())
                    .unwrap();
                let new_settings = match setting_whitout_path.strip_prefix('.') {
                    Some(s) => s,
                    None => return None, // Pas de point après le préfixe = match exact sans valeur
                };

                // Recherche récursive dans le sous-ensemble
                return Some(Self::localise_option_node_attr_set(
                    &c, new_settings, indent_level+1usize));
            } else if c.kind() == rnix::SyntaxKind::NODE_WITH {
                for children_with in c.children() {
                    match children_with.kind() {
                        rnix::SyntaxKind::NODE_LIST => {
                            return Some(SettingsPosition::ExistingOption(ExistingOption::new(
                                <TextRange as Into<Range<usize>>>::into(ast.text_range()),
                                <TextRange as Into<Range<usize>>>::into(children_with.text_range()),
                                indent_level,
                            )))
                        },
                        _ => ()
                    }
                }
                return None;
            } else {
                // Cas 2: On mets comme on peut a la fin du set
                return Some(SettingsPosition::ExistingOption(ExistingOption::new(
                    <TextRange as Into<Range<usize>>>::into(ast.text_range()),
                    <TextRange as Into<Range<usize>>>::into(c.text_range()),
                    indent_level,
                )));
            }
        }

        // Aucune valeur trouvée (cas très rare)
        None
    }
}

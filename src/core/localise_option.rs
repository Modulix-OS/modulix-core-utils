use rnix::{self, TextSize};

/// Position d'une option dans un fichier de configuration Nix.
///
/// Cette structure représente l'emplacement d'une option dans l'arbre syntaxique
/// d'un fichier Nix, avec des informations sur sa définition, sa valeur, et le
/// chemin restant à parcourir si l'option n'existe pas complètement.
///
/// # Exemples
///
/// ```ignore///
/// let source = r#"{ services.nginx.enable = true; }"#;
/// let ast = Root::parse(source).syntax();
///
/// // Rechercher une option existante
/// let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
/// assert!(position.get_remaining_path().is_none()); // Match complet
///
/// // Rechercher une option inexistante
/// let position = SettingsPosition::new(&ast, "services.apache.enable").unwrap();
/// assert_eq!(position.get_remaining_path(), Some("apache.enable"));
/// ```
#[derive(Debug, Clone)]
pub struct SettingsPosition<'a> {

    /// Position de la définition complète de l'option (clé + valeur).
    /// Pour une option existante, couvre toute la ligne d'attribution.
    /// Pour une option à créer, pointe vers l'emplacement d'insertion.
    def_option: rnix::TextRange,

    /// Position optionnelle de la valeur uniquement.
    /// `Some(range)` si l'option existe avec une valeur.
    /// `None` si l'option n'existe pas encore.
    value_option: Option<rnix::TextRange>,

    /// Chemin restant de l'option à insérer.
    /// `Some(path)` indique qu'il reste un chemin à insérer (option non trouvée).
    /// `None` indique que l'option a été complètement trouvée (match exact).
    option_path: Option<&'a str>,
}

impl<'a> SettingsPosition<'a> {

    /// Retourne la position de la définition complète de l'option.
    ///
    /// Cette position couvre soit :
    /// - L'intégralité de la ligne d'attribution pour une option existante (ex: `nginx.enable = true;`)
    /// - Le point d'insertion avant le `}` fermant pour une option inexistante
    ///
    /// # Exemples
    ///
    /// ```ignore
    /// let source = "{ enable = true; }";
    /// let ast = Root::parse(source).syntax();
    /// let position = SettingsPosition::new(&ast, "enable").unwrap();
    ///
    /// let def_range = position.get_pos_definition();
    /// println!("Définition à: {:?}", def_range);
    /// ```
    pub fn get_pos_definition(&self) -> rnix::TextRange {
        self.def_option
    }

    /// Retourne la position de la valeur de l'option, si elle existe.
    ///
    /// # Retour
    ///
    /// - `Some(TextRange)` : L'option existe et possède une valeur à cette position
    /// - `None` : L'option n'existe pas dans le fichier
    ///
    /// # Exemples
    ///
    /// ```ignore
    /// let source = r#"{ hostname = "server"; }"#;
    /// let ast = Root::parse(source).syntax();
    /// let position = SettingsPosition::new(&ast, "hostname").unwrap();
    ///
    /// if let Some(value_range) = position.get_pos_definition_value() {
    ///     let value = &source[value_range];
    ///     println!("Valeur: {}", value); // "server"
    /// }
    /// ```
    pub fn get_pos_definition_value(&self) -> Option<rnix::TextRange> {
        self.value_option
    }

    /// Retourne le chemin restant de l'option à parcourir.
    ///
    /// # Retour
    ///
    /// - `None` : L'option a été trouvée complètement (match exact)
    /// - `Some(path)` : Il reste un chemin à parcourir, `path` contient la partie manquante
    ///
    /// # Exemples
    ///
    /// ```ignore
    /// // Option existante
    /// let source = "{ services.nginx.enable = true; }";
    /// let ast = Root::parse(source).syntax();
    /// let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
    /// assert_eq!(position.get_remaining_path(), None); // Match complet
    ///
    /// // Option partiellement existante
    /// let source = "{ services = {}; }";
    /// let ast = Root::parse(source).syntax();
    /// let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
    /// assert_eq!(position.get_remaining_path(), Some("nginx.enable")); // Chemin restant
    /// ```
    pub fn get_remaining_path(&self) -> Option<&'a str> {
        self.option_path
    }

    /// Crée une nouvelle instance en localisant une option dans l'AST Nix.
    ///
    /// Cette fonction parcourt récursivement l'arbre syntaxique pour trouver
    /// l'option spécifiée par le chemin `settings`. Si l'option n'existe pas,
    /// elle retourne un point d'insertion approprié.
    ///
    /// # Arguments
    ///
    /// * `nix_ast` - Le nœud racine de l'arbre syntaxique Nix à analyser
    /// * `settings` - Le chemin de l'option recherchée, avec notation pointée (ex: `"services.nginx.enable"`)
    ///
    /// # Retour
    ///
    /// - `Some(SettingsPosition)` : L'option a été trouvée ou un point d'insertion a été identifié
    /// - `None` : Aucune correspondance n'a pu être établie (cas très rare)
    ///
    /// # Exemples
    ///
    /// ```ignore
    /// let source = r#"{
    ///     services.nginx.enable = true;
    ///     networking.hostName = "myserver";
    /// }"#;
    /// let ast = Root::parse(source).syntax();
    ///
    /// // Rechercher une option existante
    /// let pos = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
    /// assert!(pos.get_remaining_path().is_none());
    ///
    /// // Rechercher une option inexistante
    /// let pos = SettingsPosition::new(&ast, "services.apache.enable").unwrap();
    /// assert!(pos.get_remaining_path().is_some());
    /// ```
    pub fn new(nix_ast: &rnix::SyntaxNode, settings: &'a str) -> Option<Self> {
        Self::localise_option(&nix_ast, &settings)
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
    fn localise_option(ast: &rnix::SyntaxNode, settings: &'a str) -> Option<SettingsPosition<'a>> {
        return match ast.kind() {
            rnix::SyntaxKind::NODE_ATTR_SET =>
                Some(Self::localise_option_node_attr_set(&ast, &settings)),
            rnix::SyntaxKind::NODE_ATTRPATH_VALUE =>
                Self::localise_option_node_attrpath_value(&ast, &settings),
            _ => {
                for c in ast.children() {
                    if let Some(ret) = Self::localise_option(&c, settings) {
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
    fn localise_option_node_attr_set(ast: &rnix::SyntaxNode, setting: &'a str) -> SettingsPosition<'a> {
        let mut best_opt_pos: Option<SettingsPosition> = None;

        // Parcourir tous les enfants pour trouver des correspondances
        for c in ast.children() {
            let opt_pos = Self::localise_option(&c, &setting);
            if let Some(pos) = opt_pos {

                // Si match exact trouvé, retourner immédiatement
                if let None = pos.option_path {
                    return pos;
                }

                // Sinon, conserver le meilleur match (option dans la définition la plus proche)
                match &best_opt_pos {
                    None => best_opt_pos = Some(pos),
                    Some(best_pos) =>  {
                        if pos.option_path.unwrap().len() > best_pos.option_path.unwrap().len() {
                            best_opt_pos = Some(pos);
                        }
                    }
                }
            }
        }

        // Retourner le meilleur match ou un point d'insertion
        match best_opt_pos {
            Some(best_pos) => best_pos,
            None => SettingsPosition {
                // Point d'insertion avant le '}' fermant
                def_option: rnix::TextRange::at(ast.text_range().end()-TextSize::from(1), TextSize::from(0)),
                value_option: None,
                option_path: Some(setting),
            },
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
    fn localise_option_node_attrpath_value(ast: &rnix::SyntaxNode, settings: &'a str) -> Option<SettingsPosition<'a>> {
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
                    &c, new_settings));
            } else if c.kind() == rnix::SyntaxKind::NODE_WITH {
                for children_with in c.children() {
                    match children_with.kind() {
                        rnix::SyntaxKind::NODE_LIST => {
                            return Some(SettingsPosition {
                                def_option: ast.text_range(),
                                value_option: Some(children_with.text_range()),
                                option_path: None,
                            })
                        },
                        _ => return None
                    }
                }
            } else {
                // Cas 2: La valeur est une valeur simple (string, number, bool, path, etc.)
                // C'est un match exact
                return Some(SettingsPosition {
                    def_option: ast.text_range(),
                    value_option: Some(c.text_range()),
                    option_path: None,
                });
            }
        }

        // Aucune valeur trouvée (cas très rare)
        None
    }
}

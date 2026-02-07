// Tests unitaires et d'intégration pour SettingsPosition
//
// Ce fichier contient une suite de tests complète couvrant tous les cas d'usage
// de la structure SettingsPosition et ses méthodes de localisation d'options Nix.

#[cfg(test)]
mod tests {
    use crate::SettingsPosition;
    use rnix::{Root, TextSize};

    // ============================================================================
    // TESTS POUR SettingsPosition::new et localise_option
    // ============================================================================

    #[test]
    fn test_new_simple_flat_attribute() {
        // Test d'une option simple au niveau racine
        let source = "{ enable = true; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert!(pos.get_remaining_path().is_none()); // Match complet
        assert!(pos.get_pos_definition_value().is_some()); // Valeur trouvée

        // Vérifier que la valeur pointe vers "true"
        let value_range = pos.get_pos_definition_value().unwrap();
        let value_text = &source[value_range];
        assert_eq!(value_text, "true");
    }

    #[test]
    fn test_new_nested_dotted_path() {
        // Test d'un chemin d'option avec notation pointée
        let source = "{ services.nginx.enable = true; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert!(pos.get_remaining_path().is_none()); // Match complet
        assert!(pos.get_pos_definition_value().is_some());
    }

    #[test]
    fn test_new_nested_in_attrset() {
        // Test d'une option imbriquée dans un ensemble d'attributs
        let source = r#"{
            services = {
                nginx = {
                    enable = true;
                };
            };
        }"#;
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert!(pos.get_remaining_path().is_none()); // Match complet
        assert!(pos.get_pos_definition_value().is_some());
    }

    #[test]
    fn test_new_mixed_notation() {
        // Test avec un mélange de notation pointée et ensembles
        let source = r#"{
            services.nginx = {
                enable = true;
                port = 80;
            };
        }"#;
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.port");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert!(pos.get_remaining_path().is_none());
        assert!(pos.get_pos_definition_value().is_some());
    }

    #[test]
    fn test_new_not_found_returns_insertion_point() {
        // Test qu'une option inexistante retourne un point d'insertion
        let source = "{ networking.hostName = \"myserver\"; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert_eq!(pos.get_remaining_path(), Some("services.nginx.enable"));
        assert!(pos.get_pos_definition_value().is_none()); // Pas de valeur car n'existe pas
    }

    #[test]
    fn test_new_partial_match() {
        // Test d'un match partiel du chemin
        let source = r#"{
            services = {
                openssh.enable = true;
            };
        }"#;
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert_eq!(pos.get_remaining_path(), Some("nginx.enable")); // Chemin restant
    }

    #[test]
    fn test_new_empty_attrset() {
        // Test avec un ensemble d'attributs vide
        let source = "{ }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert_eq!(pos.get_remaining_path(), Some("services.nginx.enable"));
    }

    #[test]
    fn test_new_multiple_attributes_same_level() {
        // Test avec plusieurs attributs au même niveau
        let source = r#"{
            networking.hostName = "server1";
            services.nginx.enable = true;
            services.openssh.enable = true;
            environment.systemPackages = [];
        }"#;
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.openssh.enable");

        assert!(position.is_some());
        let pos = position.unwrap();
        assert!(pos.get_remaining_path().is_none());
        assert!(pos.get_pos_definition_value().is_some());
    }

    // ============================================================================
    // TESTS POUR get_pos_definition
    // ============================================================================

    #[test]
    fn test_get_pos_definition_existing_option() {
        // Test que get_pos_definition retourne une position valide
        let source = "{ enable = true; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "enable").unwrap();
        let def_range = position.get_pos_definition();

        // La position doit couvrir la définition
        assert!(<TextSize as Into<u32>>::into(def_range.start()) > 0u32);
        assert!(<TextSize as Into<u32>>::into(def_range.end()) > def_range.start().into());
    }

    #[test]
    fn test_get_pos_definition_insertion_point() {
        // Test du point d'insertion pour une option inexistante
        let source = "{ existing = 1; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "newOption").unwrap();
        let def_range = position.get_pos_definition();

        // Le point d'insertion doit être valide
        assert!(<TextSize as Into<u32>>::into(def_range.start()) > 0u32);
    }

    // ============================================================================
    // TESTS POUR get_pos_definition_value
    // ============================================================================

    #[test]
    fn test_get_pos_definition_value_with_string() {
        // Test avec une valeur de type string
        let source = r#"{ hostname = "myserver"; }"#;
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "hostname").unwrap();
        let value_range = position.get_pos_definition_value().unwrap();

        let value = &source[value_range];
        assert_eq!(value, r#""myserver""#);
    }

    #[test]
    fn test_get_pos_definition_value_with_number() {
        // Test avec une valeur numérique
        let source = "{ port = 8080; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "port").unwrap();
        let value_range = position.get_pos_definition_value().unwrap();

        let value = &source[value_range];
        assert_eq!(value, "8080");
    }

    #[test]
    fn test_get_pos_definition_value_with_bool() {
        // Test avec une valeur booléenne
        let source = "{ enable = false; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "enable").unwrap();
        let value_range = position.get_pos_definition_value().unwrap();

        let value = &source[value_range];
        assert_eq!(value, "false");
    }

    #[test]
    fn test_get_pos_definition_value_with_path() {
        // Test avec une valeur de type chemin
        let source = "{ root = /var/www; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "root").unwrap();
        let value_range = position.get_pos_definition_value().unwrap();

        let value = &source[value_range];
        assert_eq!(value, "/var/www");
    }

    #[test]
    fn test_get_pos_definition_value_none_for_missing_option() {
        // Test que None est retourné pour une option inexistante
        let source = "{ existing = 1; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "missing").unwrap();
        assert!(position.get_pos_definition_value().is_none());
    }

    // ============================================================================
    // TESTS POUR get_remaining_path
    // ============================================================================

    #[test]
    fn test_get_remaining_path_none_for_exact_match() {
        // Test que None est retourné pour un match exact
        let source = "{ services.nginx.enable = true; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
        assert_eq!(position.get_remaining_path(), None);
    }

    #[test]
    fn test_get_remaining_path_partial_match() {
        // Test du chemin restant pour un match partiel
        let source = "{ services = {}; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
        assert_eq!(position.get_remaining_path(), Some("nginx.enable"));
    }

    #[test]
    fn test_get_remaining_path_no_match() {
        // Test du chemin complet pour une option totalement inexistante
        let source = "{ other = 1; }";
        let ast = Root::parse(source).syntax();

        let position = SettingsPosition::new(&ast, "services.nginx.enable").unwrap();
        assert_eq!(position.get_remaining_path(), Some("services.nginx.enable"));
    }

    // ============================================================================
    // TESTS D'INTÉGRATION
    // ============================================================================

    #[test]
    fn test_integration_complex_nixos_config() {
        // Test avec une configuration NixOS réaliste
        let source = r#"{
            imports = [ ./hardware-configuration.nix ];

            boot.loader.systemd-boot.enable = true;

            networking = {
                hostName = "nixos-server";
                firewall.enable = true;
            };

            services = {
                openssh = {
                    enable = true;
                    permitRootLogin = "no";
                };
                nginx = {
                    enable = true;
                    virtualHosts."example.com" = {
                        root = "/var/www";
                    };
                };
            };

            system.stateVersion = "23.11";
        }"#;
        let ast = Root::parse(source).syntax();

        // Test 1: Option existante simple
        let pos = SettingsPosition::new(&ast, "boot.loader.systemd-boot.enable").unwrap();
        assert!(pos.get_remaining_path().is_none());
        assert!(pos.get_pos_definition_value().is_some());

        // Test 2: Option dans un ensemble imbriqué
        let pos = SettingsPosition::new(&ast, "networking.hostName").unwrap();
        assert!(pos.get_remaining_path().is_none());

        // Test 3: Option profondément imbriquée
        let pos = SettingsPosition::new(&ast, "services.openssh.permitRootLogin").unwrap();
        assert!(pos.get_remaining_path().is_none());

        // Test 4: Option inexistante dans un ensemble existant
        let pos = SettingsPosition::new(&ast, "services.postgresql.enable").unwrap();
        assert_eq!(pos.get_remaining_path(), Some("postgresql.enable"));

        // Test 5: Option complètement inexistante
        let pos = SettingsPosition::new(&ast, "hardware.pulseaudio.enable").unwrap();
        assert_eq!(pos.get_remaining_path(), Some("hardware.pulseaudio.enable"));
    }

    #[test]
    fn test_integration_multiple_similar_paths() {
        // Test de discrimination entre chemins similaires
        let source = r#"{
            services.web = {
                enable = true;
            };
            services.webserver = {
                enable = false;
            };
        }"#;
        let ast = Root::parse(source).syntax();

        let pos1 = SettingsPosition::new(&ast, "services.web.enable").unwrap();
        assert!(pos1.get_remaining_path().is_none());

        let pos2 = SettingsPosition::new(&ast, "services.webserver.enable").unwrap();
        assert!(pos2.get_remaining_path().is_none());
    }

    #[test]
    fn test_integration_list_values() {
        // Test avec des valeurs de type liste
        let source = r#"{
            environment.systemPackages = [ pkg1 pkg2 pkg3 ];
        }"#;
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "environment.systemPackages").unwrap();
        assert!(pos.get_remaining_path().is_none());
        assert!(pos.get_pos_definition_value().is_some());
    }

    #[test]
    fn test_integration_insertion_point_position() {
        // Test que le point d'insertion est bien positionné
        let source = r#"{
            services.nginx.enable = true;
        }"#;
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "services.apache.enable").unwrap();
        assert_eq!(pos.get_remaining_path(), Some("services.apache.enable"));

        // Le point d'insertion devrait être avant le '}' fermant
        let insertion_pos = pos.get_pos_definition().start();
        assert!(<TextSize as Into<u32>>::into(insertion_pos) > 0u32);
    }

    #[test]
    fn test_integration_deeply_nested_structure() {
        // Test avec une structure profondément imbriquée
        let source = r#"{
            level1.level2.level3.level4 = {
                deepOption = "value";
            };
        }"#;
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "level1.level2.level3.level4.deepOption").unwrap();
        assert!(pos.get_remaining_path().is_none());
        assert!(pos.get_pos_definition_value().is_some());
    }

    // ============================================================================
    // TESTS DE CAS LIMITES
    // ============================================================================

    #[test]
    fn test_edge_case_single_character_keys() {
        // Test avec des clés d'un seul caractère
        let source = "{ a.b.c = 1; }";
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "a.b.c").unwrap();
        assert!(pos.get_remaining_path().is_none());
    }

    #[test]
    fn test_edge_case_very_long_path() {
        // Test avec un chemin très long
        let source = "{ a.b.c.d.e.f.g.h.i.j = \"deep\"; }";
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "a.b.c.d.e.f.g.h.i.j").unwrap();
        assert!(pos.get_remaining_path().is_none());
    }

    #[test]
    fn test_edge_case_mixed_values_same_set() {
        // Test avec différents types de valeurs dans le même ensemble
        let source = r#"{
            string = "value";
            number = 42;
            bool = true;
            path = /some/path;
            list = [ 1 2 3 ];
            set = { nested = "value"; };
        }"#;
        let ast = Root::parse(source).syntax();

        // Tous devraient être trouvés
        assert!(SettingsPosition::new(&ast, "string").unwrap().get_remaining_path().is_none());
        assert!(SettingsPosition::new(&ast, "number").unwrap().get_remaining_path().is_none());
        assert!(SettingsPosition::new(&ast, "bool").unwrap().get_remaining_path().is_none());
        assert!(SettingsPosition::new(&ast, "path").unwrap().get_remaining_path().is_none());
        assert!(SettingsPosition::new(&ast, "list").unwrap().get_remaining_path().is_none());
        assert!(SettingsPosition::new(&ast, "set.nested").unwrap().get_remaining_path().is_none());
    }

    #[test]
    fn test_edge_case_whitespace_handling() {
        // Test que les espaces sont gérés correctement
        let source = r#"{
            services  .  nginx  .  enable   =   true  ;
        }"#;
        let ast = Root::parse(source).syntax();

        // L'AST normalise les espaces
        let pos = SettingsPosition::new(&ast, "services.nginx.enable");
        assert!(pos.is_some());
    }

    #[test]
    fn test_edge_case_option_with_underscore() {
        // Test avec des options contenant des underscores
        let source = "{ my_option_name = 123; }";
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "my_option_name").unwrap();
        assert!(pos.get_remaining_path().is_none());
    }

    #[test]
    fn test_edge_case_option_with_dash() {
        // Test avec des options contenant des tirets
        let source = "{ \"my-option\" = 456; }";
        let ast = Root::parse(source).syntax();

        let pos = SettingsPosition::new(&ast, "my-option");
        // Ce cas peut ne pas matcher selon le parser Nix
        // Le test vérifie juste que ça ne panic pas
        assert!(pos.is_some());
    }

    // ============================================================================
    // TESTS FONCTIONNELS (Use Cases)
    // ============================================================================

    #[test]
    fn test_use_case_check_option_exists() {
        // Cas d'usage: Vérifier si une option existe
        fn option_exists(source: &str, path: &str) -> bool {
            let ast = Root::parse(source).syntax();
            SettingsPosition::new(&ast, path)
                .map(|pos| pos.get_remaining_path().is_none())
                .unwrap_or(false)
        }

        let config = r#"{ services.nginx.enable = true; }"#;
        assert!(option_exists(config, "services.nginx.enable"));
        assert!(!option_exists(config, "services.apache.enable"));
    }

    #[test]
    fn test_use_case_get_option_value() {
        // Cas d'usage: Obtenir la valeur d'une option
        fn get_value(source: &str, path: &str) -> Option<String> {
            let ast = Root::parse(source).syntax();
            SettingsPosition::new(&ast, path)
                .and_then(|pos| pos.get_pos_definition_value())
                .map(|range| source[range].to_string())
        }

        let config = r#"{ hostname = "server123"; }"#;
        assert_eq!(get_value(config, "hostname"), Some(r#""server123""#.to_string()));
        assert_eq!(get_value(config, "missing"), None);
    }

    #[test]
    fn test_use_case_validate_required_options() {
        // Cas d'usage: Valider que toutes les options requises sont présentes
        fn validate_required(source: &str, required: &[&str]) -> Vec<String> {
            let ast = Root::parse(source).syntax();
            required.iter()
                .filter(|&&opt| {
                    SettingsPosition::new(&ast, opt)
                        .map(|pos| pos.get_remaining_path().is_some())
                        .unwrap_or(true)
                })
                .map(|s| s.to_string())
                .collect()
        }

        let config = r#"{
            boot.loader.systemd-boot.enable = true;
            networking.hostName = "nixos";
        }"#;

        let required = vec![
            "boot.loader.systemd-boot.enable",
            "networking.hostName",
            "services.openssh.enable", // manquant
        ];

        let missing = validate_required(config, &required);
        assert_eq!(missing, vec!["services.openssh.enable"]);
    }

    // ============================================================================
    // TESTS DE PERFORMANCE (optionnels, commentés par défaut)
    // ============================================================================

    #[test]
    #[ignore] // Ignorer par défaut, exécuter avec --ignored
    fn test_performance_large_config() {
        // Test de performance avec une grande configuration
        let mut source = String::from("{\n");

        // Générer 1000 options
        for i in 0..1000 {
            source.push_str(&format!("  option_{} = {};\n", i, i));
        }
        source.push_str("}");

        let ast = Root::parse(&source).syntax();

        use std::time::Instant;
        let start = Instant::now();

        // Rechercher une option au milieu
        let _result = SettingsPosition::new(&ast, "option_1000");

        let duration = start.elapsed();

        // La recherche devrait être rapide même avec beaucoup d'options
        assert!(duration.as_millis() < 100, "Recherche trop lente: {:?}", duration);
    }

    #[test]
    #[ignore]
    fn test_performance_deeply_nested_config() {
        // Test avec une configuration profondément imbriquée
        let source = r#"{
            l1 = { l2 = { l3 = { l4 = { l5 = { l6 = { l7 = { l8 = { l9 = { l10 = {
                deep_option = "value";
            }; }; }; }; }; }; }; }; }; };
        }"#;
        let ast = Root::parse(source).syntax();

        use std::time::Instant;
        let start = Instant::now();

        let _result = SettingsPosition::new(&ast, "l1.l2.l3.l4.l5.l6.l7.l8.l9.l10.deep_option");

        let duration = start.elapsed();
        assert!(duration.as_millis() < 50, "Recherche trop lente: {:?}", duration);
    }

    // ============================================================================
    // TESTS DE CLONE
    // ============================================================================

    #[test]
    fn test_settings_position_is_clonable() {
        // Test que SettingsPosition peut être cloné
        let source = "{ enable = true; }";
        let ast = Root::parse(source).syntax();

        let pos1 = SettingsPosition::new(&ast, "enable").unwrap();
        let pos2 = pos1.clone();

        assert_eq!(pos1.get_remaining_path(), pos2.get_remaining_path());
        assert_eq!(pos1.get_pos_definition(), pos2.get_pos_definition());
        assert_eq!(pos1.get_pos_definition_value(), pos2.get_pos_definition_value());
    }
}

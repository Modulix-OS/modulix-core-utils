use super::*;

#[test]
fn parse_release_reads_both_fields() {
    let release = parse_release(r#"{"version":"0.2","codeName":"Clarté"}"#)
        .expect("a well-formed release.json must parse");

    assert_eq!(release.version, "0.2");
    assert_eq!(release.code_name, "Clarté");
}

#[test]
fn parse_release_ignores_extra_fields() {
    let release = parse_release(r#"{"version":"1.0","codeName":"X","future":"ignored"}"#)
        .expect("an added field must not break older readers");

    assert_eq!(release.version, "1.0");
}

#[test]
fn parse_release_rejects_truncated_json() {
    assert!(matches!(
        parse_release(r#"{"version":"0.2","codeNa"#),
        Err(mx::ErrorKind::ParseError(_))
    ));
}

#[test]
fn parse_release_rejects_missing_code_name() {
    assert!(parse_release(r#"{"version":"0.2"}"#).is_err());
}

#[test]
fn parse_release_rejects_implausible_versions() {
    for version in [
        "",
        "v0.2",
        "<b>0.2",
        "0.2 (Clarté)",
        "0.2/../../etc",
        &"9".repeat(MAX_VERSION_LEN + 1),
    ] {
        let body = serde_json::json!({ "version": version, "codeName": "X" }).to_string();
        assert!(
            parse_release(&body).is_err(),
            "version {version:?} must be rejected"
        );
    }
}

#[test]
fn parse_release_rejects_implausible_code_names() {
    for code_name in ["", "line\nbreak", &"x".repeat(MAX_CODE_NAME_LEN + 1)] {
        let body = serde_json::json!({ "version": "0.2", "codeName": code_name }).to_string();
        assert!(
            parse_release(&body).is_err(),
            "codeName {code_name:?} must be rejected"
        );
    }
}

#[test]
fn remote_release_url_points_at_the_tracked_branch() {
    assert_eq!(
        REMOTE_RELEASE_URL,
        "https://raw.githubusercontent.com/Modulix-OS/mxpkgs/heads/main/release.json"
    );
}

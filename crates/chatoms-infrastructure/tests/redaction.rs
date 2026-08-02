use chatoms_infrastructure::redaction::{
    MAX_REDACTION_INPUT_BYTES, REDACTED_MARKER, REDACTION_FAILED_MARKER, SecretRedactor,
    TRUNCATED_MARKER,
};

fn redactor() -> SecretRedactor {
    SecretRedactor::new().expect("static redaction rules must compile")
}

fn assert_secret_removed(input: &str, secret: &str) {
    let report = redactor().redact_text(input);
    assert!(!report.text.as_str().contains(secret), "{input}");
    assert!(report.replacement_count > 0, "{input}");
}

#[test]
fn headers_key_values_cli_options_and_cookies_are_redacted() {
    for (input, secret) in [
        ("Authorization: Bearer bearer-secret", "bearer-secret"),
        ("Authorization: Basic YmFzaWMtc2VjcmV0", "YmFzaWMtc2VjcmV0"),
        ("aUtHoRiZaTiOn: bEaReR mixed-secret", "mixed-secret"),
        ("api_key=api-secret", "api-secret"),
        (r#"{\"token\":\"json-secret\"}"#, "json-secret"),
        ("tool --client-secret cli-secret", "cli-secret"),
        ("Cookie: sid=cookie-secret", "cookie-secret"),
        ("Set-Cookie: sid=set-cookie-secret", "set-cookie-secret"),
        ("session_id=session-secret", "session-secret"),
        (
            "https://example.test/?access_token=query-secret",
            "query-secret",
        ),
    ] {
        assert_secret_removed(input, secret);
    }
}

#[test]
fn private_keys_and_provider_tokens_are_redacted() {
    for (input, secret) in [
        (
            "-----BEGIN PRIVATE KEY-----\nprivate-secret\n-----END PRIVATE KEY-----",
            "private-secret",
        ),
        (
            "-----BEGIN PRIVATE KEY-----\r\ntruncated-private-secret",
            "truncated-private-secret",
        ),
        (
            "token=abcdefgh.ijklmnop.qrstuvwx",
            "abcdefgh.ijklmnop.qrstuvwx",
        ),
        (
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
        ),
        (
            "github_pat_abcdefghijklmnopqrstuvwxyz012345",
            "github_pat_abcdefghijklmnopqrstuvwxyz012345",
        ),
        (
            "sk-abcdefghijklmnopqrstuvwxyz",
            "sk-abcdefghijklmnopqrstuvwxyz",
        ),
        ("https://alice:url-secret@example.test/path", "url-secret"),
    ] {
        assert_secret_removed(input, secret);
    }
}

#[test]
fn encoded_secrets_fail_closed() {
    for input in [
        "api_key%3Dpercent-secret",
        r#"{\"token\":\"escaped-secret\"}"#,
    ] {
        let report = redactor().redact_text(input);
        assert!(report.failed_closed, "{input}");
        assert!(report.text.as_str().starts_with(REDACTION_FAILED_MARKER));
        assert!(!report.text.as_str().contains("secret"));
    }
}

#[test]
fn multiline_and_multiple_secrets_are_all_redacted() {
    let input = "Authorization: Bearer first\r\npassword=second\nCookie: sid=third";
    let report = redactor().redact_text(input);
    assert!(report.replacement_count >= 3);
    for secret in ["first", "second", "third"] {
        assert!(!report.text.as_str().contains(secret));
    }
}

#[test]
fn non_sensitive_text_and_ordinary_token_word_are_unchanged() {
    for input in [
        "ordinary diagnostic message",
        "the token budget remains bounded",
    ] {
        let report = redactor().redact_text(input);
        assert_eq!(report.text.as_str(), input);
        assert_eq!(report.replacement_count, 0);
    }
}

#[test]
fn output_is_deterministic_and_bounded() {
    let redactor = redactor();
    let input = format!(
        "password=hidden {}",
        "x".repeat(MAX_REDACTION_INPUT_BYTES + 64)
    );
    let first = redactor.redact_text(&input);
    let second = redactor.redact_text(&input);
    assert_eq!(first, second);
    assert!(first.truncated);
    assert!(first.text.as_str().ends_with(TRUNCATED_MARKER));
    assert!(first.text.as_str().len() <= MAX_REDACTION_INPUT_BYTES + TRUNCATED_MARKER.len());
}

#[test]
fn debug_display_and_markers_never_expose_original_secret() {
    let report = redactor().redact_text("password=display-secret");
    let displayed = report.text.to_string();
    let debugged = format!("{:?}", report.text);
    assert!(!displayed.contains("display-secret"));
    assert!(!debugged.contains("display-secret"));
    assert!(displayed.contains(REDACTED_MARKER));
    assert!(!redactor().contains_sensitive_material(REDACTED_MARKER));
    assert!(!redactor().contains_sensitive_material(REDACTION_FAILED_MARKER));
}

#[test]
fn sensitive_fields_are_masked_and_raw_validation_fails_closed() {
    let redactor = redactor();
    let report = redactor.redact_field("client_secret", "unstructured-value");
    assert_eq!(report.text.as_str(), REDACTED_MARKER);
    assert!(redactor.validate_redacted("api_key=still-secret").is_err());
    assert_eq!(
        redactor
            .validate_redacted("safe value")
            .expect("safe text must validate")
            .as_str(),
        "safe value"
    );
}

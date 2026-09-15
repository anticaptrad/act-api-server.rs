use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn policy() -> String {
    fs::read_to_string(root().join(".cli-flags.toml")).expect("read policy")
}

#[test]
fn policy_is_fail_closed_and_dotenv_free() {
    let text = policy();
    assert!(text.contains("allow_unknown = false"));
    assert!(text.contains("load = false"));
}

#[test]
fn policy_uses_aliases_not_retired_long_keys() {
    let text = policy();
    assert!(
        !text
            .lines()
            .any(|line| line.trim_start().starts_with("long ="))
    );
    assert!(text.contains("aliases = [\"port\"]"));
    assert!(text.contains("aliases = [\"shared-auth-url\"]"));
}

#[test]
fn sensitive_values_remain_environment_only() {
    let text = policy();
    for key in [
        "ACT_API_TLS_KEY_FILE",
        "ACT_NATS_OPERATION_HMAC_KEY",
        "ACT_OPERATION_DATABASE_URL",
        "NATS_URL",
        "SHARED_AUTH_SERVICE_CREDENTIAL",
        "YOUTUBE_GAS_API_KEY",
    ] {
        assert!(text.contains(key), "missing environment-only key {key}");
    }
}

//! Token order for the session events API (s461): env, token file, macOS keychain, Claude Code login file.
//! Fixtures live under CARGO_TARGET_TMPDIR (the target folder); the process environment is not changed.
use remote_sessions_cli::{resolve_token, LOGIN_EXPIRED, NO_TOKEN};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_800_000_000_000;

fn home(name: &str) -> PathBuf {
    // A new folder per run (nothing is deleted); files in it are written fresh.
    let run = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("s461-token").join(run).join(name);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::create_dir_all(dir.join(".config/remote-sessions")).unwrap();
    dir
}
fn creds(home: &Path, text: &str) { std::fs::write(home.join(".claude/.credentials.json"), text).unwrap(); }
fn login(token: &str, expires: i64) -> String {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"refresh-never-used","expiresAt":{expires},"refreshTokenExpiresAt":{expires},"scopes":["user:inference","user:sessions:claude_code"],"subscriptionType":"max","rateLimitTier":"x","subscriptionTypeSource":"x"}}}}"#)
}
fn resolve(vars: &[(&str, String)], keychain: Option<&str>) -> Result<String, String> {
    let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let kc = keychain.map(str::to_string);
    resolve_token(&|k| map.get(k).cloned(), &|| kc.clone(), NOW)
}
fn h(home: &Path) -> (&'static str, String) { ("HOME", home.display().to_string()) }

#[test]
fn linux_login_file_is_used_when_nothing_else_is_set() {
    let home = home("valid");
    creds(&home, &login("login-tok-1", NOW + 3_600_000));
    assert_eq!(resolve(&[h(&home)], None).unwrap(), "login-tok-1");
}

#[test]
fn expired_login_says_so_and_does_not_send_it() {
    let home = home("expired");
    creds(&home, &login("login-tok-old", NOW - 1));
    let err = resolve(&[h(&home)], None).unwrap_err();
    assert_eq!(err, LOGIN_EXPIRED);
    assert!(err.contains("run claude once"));
    assert!(!err.contains("login-tok-old"));
}

#[test]
fn missing_access_token_key_is_a_clear_error() {
    let home = home("nokey");
    creds(&home, r#"{"claudeAiOauth":{"refreshToken":"refresh-secret","expiresAt":99999999999999}}"#);
    let err = resolve(&[h(&home)], None).unwrap_err();
    assert!(err.contains("no claudeAiOauth.accessToken"), "{err}");
    assert!(!err.contains("refresh-secret"));
    creds(&home, r#"{"accessToken":"top-level-is-not-a-claude-login"}"#);
    assert!(resolve(&[h(&home)], None).unwrap_err().contains("no claudeAiOauth.accessToken"));
}

#[test]
fn bad_json_is_a_clear_error_without_the_file_text() {
    let home = home("badjson");
    creds(&home, "{\"claudeAiOauth\": {\"accessToken\": \"half-a-secret");
    let err = resolve(&[h(&home)], None).unwrap_err();
    assert!(err.contains("not valid JSON"), "{err}");
    assert!(!err.contains("half-a-secret"));
}

#[test]
fn missing_file_gives_the_no_token_error_that_names_the_linux_file() {
    let home = home("missing");
    let err = resolve(&[h(&home)], None).unwrap_err();
    assert_eq!(err, NO_TOKEN);
    assert!(err.contains("~/.claude/.credentials.json") && err.contains("CLAUDE_CONFIG_DIR"));
    assert_eq!(resolve(&[], None).unwrap_err(), NO_TOKEN);
}

#[test]
fn wrong_file_mode_is_accepted() {
    // Claude Code writes the file 0600; a wider mode is accepted (the file is only read, never written).
    use std::os::unix::fs::PermissionsExt;
    let home = home("mode");
    creds(&home, &login("login-tok-mode", NOW + 1));
    std::fs::set_permissions(home.join(".claude/.credentials.json"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(resolve(&[h(&home)], None).unwrap(), "login-tok-mode");
}

#[test]
fn claude_config_dir_moves_the_login_file() {
    let home = home("configdir");
    creds(&home, &login("default-dir-tok", NOW + 1));
    let other = home.join("claude-work");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join(".credentials.json"), login("work-dir-tok", NOW + 1)).unwrap();
    assert_eq!(resolve(&[h(&home), ("CLAUDE_CONFIG_DIR", other.display().to_string())], None).unwrap(), "work-dir-tok");
    // Set but empty: ignored, the default folder is used.
    assert_eq!(resolve(&[h(&home), ("CLAUDE_CONFIG_DIR", String::new())], None).unwrap(), "default-dir-tok");
}

#[test]
fn precedence_env_then_token_file_then_keychain_then_login_file() {
    let home = home("order");
    creds(&home, &login("login-file-tok", NOW + 1));
    std::fs::write(home.join(".config/remote-sessions/token"), "file-tok\n").unwrap();
    assert_eq!(resolve(&[h(&home), ("REMOTE_SESSIONS_TOKEN", "env-tok".into())], Some("keychain-tok")).unwrap(), "env-tok");
    assert_eq!(resolve(&[h(&home), ("REMOTE_SESSIONS_TOKEN", "  ".into())], Some("keychain-tok")).unwrap(), "file-tok");
    assert_eq!(resolve(&[h(&home)], Some("keychain-tok")).unwrap(), "file-tok");
    std::fs::rename(home.join(".config/remote-sessions/token"), home.join(".config/remote-sessions/token.moved")).unwrap();
    assert_eq!(resolve(&[h(&home)], Some("keychain-tok")).unwrap(), "keychain-tok");
    assert_eq!(resolve(&[h(&home)], None).unwrap(), "login-file-tok");
    // An empty token file falls through too.
    std::fs::write(home.join(".config/remote-sessions/token"), "\n").unwrap();
    assert_eq!(resolve(&[h(&home)], None).unwrap(), "login-file-tok");
}

mod common;

use common::*;

#[test]
fn settings_get() -> Result<()> {
    cli()
        .args(["settings", "test-value"])
        .assert()
        .code(predicate::in_iter([0, 1]));

    cli()
        .args(["settings", "test-value", "-f", "json"])
        .assert()
        .stdout(is_json())
        .success();
    Ok(())
}

#[test]
fn settings_bare_prints_overview() -> Result<()> {
    cli()
        .arg("settings")
        .env_remove("Q_LOG_STDOUT")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("Settings file: ").and(predicate::str::contains("q settings open")));
    Ok(())
}

mod common;

use common::*;

// Integrations tests for the CLI
//
// This should be used to test interfaces that external code may rely on
// (exit codes, structured output, CLI flags)

#[test]
fn version_flag_has_status_code_zero() -> Result<()> {
    cli()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    Ok(())
}

#[test]
fn version_subcommand_has_status_code_zero() -> Result<()> {
    cli()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    Ok(())
}

#[test]
fn help_flag_has_status_code_zero() -> Result<()> {
    cli().arg("--help").assert().success();
    Ok(())
}

#[test]
fn no_args_prints_help_with_status_code_zero() -> Result<()> {
    cli().assert().success().stdout(predicate::str::contains("Usage"));
    Ok(())
}

#[test]
fn help_all_flag_has_status_code_zero() -> Result<()> {
    cli().arg("--help-all").assert().success();
    Ok(())
}

#[test]
fn update_prints_not_available_with_status_code_zero() -> Result<()> {
    for name in ["update", "upgrade"] {
        cli()
            .arg(name)
            .env_remove("SHELLCUE_LOG_STDOUT")
            .assert()
            .success()
            .stdout("Updating is not available in this build yet.\n");
    }
    Ok(())
}

use std::process::{Command, Stdio};

use uriel_mcp::config::{parse_args_envs, Cli, Config, Terse, TERSE_ENV, USAGE};

fn run(args: &[&str], env: Option<&str>) -> Config {
    match parse_args_envs(args.iter().copied(), None, env).expect("args") {
        Cli::Run(c) => c,
        other => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn the_default_is_off() {
    assert_eq!(Terse::default(), Terse::Off);
    assert_eq!(run(&[], None).terse, Terse::Off);
    assert_eq!(run(&[], Some("")).terse, Terse::Off);
}

#[test]
fn the_flag_selects_the_level_in_both_spellings() {
    for (word, level) in [("off", Terse::Off), ("data", Terse::Data), ("prose", Terse::Prose)] {
        assert_eq!(run(&["--terse", word], None).terse, level);
        assert_eq!(run(&[&format!("--terse={word}")], None).terse, level);
        assert_eq!(Terse::parse(word), Some(level));
        assert_eq!(level.name(), word);
    }
}

#[test]
fn the_environment_is_used_only_without_a_flag() {
    assert_eq!(run(&[], Some("data")).terse, Terse::Data);
    assert_eq!(run(&[], Some("prose")).terse, Terse::Prose);
    assert_eq!(run(&["--terse", "off"], Some("prose")).terse, Terse::Off);
    assert_eq!(run(&["--terse", "prose"], Some("off")).terse, Terse::Prose);
}

#[test]
fn a_bad_flag_or_environment_value_is_an_error_that_names_its_source() {
    for bad in [&["--terse", "full"][..], &["--terse"], &["--terse="], &["--terse", "Data"], &["--terse", "on"]] {
        let e = parse_args_envs(bad.iter().copied(), None, None).expect_err("must fail");
        assert!(e.contains("--terse"), "{bad:?}: {e}");
    }
    let e = parse_args_envs(Vec::<String>::new(), None, Some("full")).expect_err("must fail");
    assert!(e.contains(TERSE_ENV), "{e}");
    assert!(parse_args_envs(["--terse", "data"], None, Some("full")).is_ok());
}

#[test]
fn the_usage_documents_the_flag_and_the_environment() {
    assert!(USAGE.contains("--terse <L>"), "{USAGE}");
    assert!(USAGE.contains("URIEL_TERSE"), "{USAGE}");
}

fn exit_code(args: &[&str], env: Option<&str>) -> Option<i32> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_uriel-mcp"));
    cmd.args(args)
        .arg("--in-memory")
        .env_remove("URIEL_MODE")
        .env_remove(TERSE_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(v) = env {
        cmd.env(TERSE_ENV, v);
    }
    cmd.status().expect("spawn").code()
}

#[test]
fn a_bad_value_exits_with_status_two() {
    assert_eq!(exit_code(&["--terse", "full"], None), Some(2));
    assert_eq!(exit_code(&[], Some("full")), Some(2));
    assert_eq!(exit_code(&["--terse", "data"], Some("full")), Some(0));
}

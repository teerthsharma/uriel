use uriel_mcp::config::{parse_args_env, Cli, Config, Mode, USAGE};
use uriel_mcp::tools::list_json;

fn run(args: &[&str], env: Option<&str>) -> Config {
    match parse_args_env(args.iter().copied(), env).expect("args") {
        Cli::Run(c) => c,
        other => panic!("expected Run, got {other:?}"),
    }
}

fn names(read_only: bool, mode: Mode) -> Vec<String> {
    list_json(read_only, mode)["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect()
}

#[test]
fn the_default_is_normal() {
    assert_eq!(Mode::default(), Mode::Normal);
    assert_eq!(run(&[], None).mode, Mode::Normal);
}

#[test]
fn the_flag_selects_the_mode_in_both_spellings() {
    assert_eq!(run(&["--mode", "house"], None).mode, Mode::House);
    assert_eq!(run(&["--mode=house"], None).mode, Mode::House);
    assert_eq!(run(&["--mode", "normal"], None).mode, Mode::Normal);
    assert_eq!(run(&["--mode=normal"], None).mode, Mode::Normal);
}

#[test]
fn the_environment_is_used_only_without_a_flag() {
    assert_eq!(run(&[], Some("house")).mode, Mode::House);
    assert_eq!(run(&[], Some("normal")).mode, Mode::Normal);
    assert_eq!(run(&[], Some("")).mode, Mode::Normal);
    assert_eq!(run(&["--mode", "normal"], Some("house")).mode, Mode::Normal);
    assert_eq!(run(&["--mode", "house"], Some("normal")).mode, Mode::House);
}

#[test]
fn a_bad_flag_or_environment_value_is_an_error_that_names_its_source() {
    for bad in [&["--mode", "warp"][..], &["--mode"], &["--mode="], &["--mode", "House"], &["--mode", "seal"]] {
        let e = parse_args_env(bad.iter().copied(), None).expect_err("must fail");
        assert!(e.contains("--mode"), "{bad:?}: {e}");
    }
    let e = parse_args_env(Vec::<String>::new(), Some("warp")).expect_err("must fail");
    assert!(e.contains("URIEL_MODE"), "{e}");
    assert!(parse_args_env(["--mode", "normal"], Some("warp")).is_ok());
}

#[test]
fn mode_names_round_trip() {
    for m in [Mode::Normal, Mode::House] {
        assert_eq!(Mode::parse(m.name()), Some(m));
    }
    assert_eq!(Mode::Normal.name(), "normal");
    assert_eq!(Mode::House.name(), "house");
}

#[test]
fn usage_documents_the_flag_and_the_variable() {
    assert!(USAGE.contains("--mode"));
    assert!(USAGE.contains("URIEL_MODE"));
}

#[test]
fn normal_lists_no_house_or_research_tool_in_either_write_mode() {
    for read_only in [false, true] {
        for n in names(read_only, Mode::Normal) {
            assert!(!n.starts_with("house_") && !n.starts_with("research_"), "{n}");
        }
    }
}

#[test]
fn house_lists_every_normal_tool_plus_fourteen_more() {
    let normal = names(false, Mode::Normal);
    let house = names(false, Mode::House);
    assert_eq!(house.len(), normal.len() + 14);
    for n in &normal {
        assert!(house.contains(n), "{n}");
    }
    assert_eq!(house.iter().filter(|n| n.starts_with("research_")).count(), 3);
    assert_eq!(house.iter().filter(|n| n.starts_with("house_")).count(), 11);
}

#[test]
fn the_listing_order_is_stable() {
    assert_eq!(names(false, Mode::House), names(false, Mode::House));
    assert_eq!(list_json(false, Mode::Normal), list_json(false, Mode::Normal));
}

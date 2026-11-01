mod common;

use common::*;
use uriel_dispatch::*;

fn adm(p: &Policy, t: &Trigger, ticket: bool) -> Result<(Strength, Basis), Refusal> {
    gate(p, t, ticket).map(|a| (a.strength, a.basis))
}

#[test]
fn a_listed_person_addressing_the_agent_is_explicit_in_every_mode() {
    for mode in [AcceptMode::Off, AcceptMode::Observe, AcceptMode::Auto] {
        let p = with_mode(mode);
        let t = person("1", "alice", true, "th");
        assert_eq!(adm(&p, &t, false), Ok((Strength::Explicit, Basis::Listed)));
    }
}

#[test]
fn an_unaddressed_message_needs_a_scope_that_receives_everything() {
    let p = policy();
    let t = person("1", "alice", false, "th");
    assert_eq!(adm(&p, &t, false), Err(Refusal::NotReceivedAll));
    let t = trigger("2", Some(("alice", false)), false, "th", "all");
    assert_eq!(
        adm(&p, &t, false),
        Ok((Strength::Inferred, Basis::Table(CellRef::ListedUnaddressed)))
    );
}

#[test]
fn an_unlisted_person_addressing_the_agent_is_ignored_by_default() {
    let t = person("1", "mallory", true, "th");
    assert_eq!(adm(&policy(), &t, false), Err(Refusal::Ignored));
}

#[test]
fn an_explicit_cell_lets_an_unlisted_person_request_work_even_with_the_mode_off() {
    let mut p = with_mode(AcceptMode::Off);
    let scope = p
        .sources
        .get_mut("gh")
        .unwrap()
        .scopes
        .get_mut("issues")
        .unwrap();
    scope.admission.unlisted_addressed = Cell::Explicit;
    let t = person("1", "mallory", true, "th");
    assert_eq!(
        adm(&p, &t, false),
        Ok((Strength::Explicit, Basis::Table(CellRef::UnlistedAddressed)))
    );
    assert_eq!(
        adm(&p, &t, true),
        Ok((Strength::Explicit, Basis::Table(CellRef::UnlistedAddressed)))
    );
}

#[test]
fn a_bot_is_never_explicit_and_must_be_listed() {
    let p = with_mode(AcceptMode::Auto);
    let unlisted = bot("1", "evil[bot]", true, "th");
    assert_eq!(adm(&p, &unlisted, false), Err(Refusal::BotNotListed));
    let listed = bot("2", "dependabot[bot]", true, "th");
    assert_eq!(
        adm(&p, &listed, false),
        Ok((Strength::Inferred, Basis::Bot))
    );
    let quiet = bot("3", "dependabot[bot]", false, "th");
    assert_eq!(adm(&p, &quiet, false), Err(Refusal::BotNotAddressed));
}

#[test]
fn a_listed_bot_posting_where_everything_is_received_is_inferred() {
    let p = with_mode(AcceptMode::Observe);
    let t = trigger("1", Some(("dependabot[bot]", true)), false, "th", "all");
    assert_eq!(adm(&p, &t, false), Ok((Strength::Inferred, Basis::Bot)));
}

#[test]
fn mode_off_refuses_every_inferred_case() {
    let p = with_mode(AcceptMode::Off);
    let bot_t = bot("1", "dependabot[bot]", true, "th");
    assert_eq!(adm(&p, &bot_t, false), Err(Refusal::ModeOff));
    let listed = trigger("2", Some(("alice", false)), false, "th", "all");
    assert_eq!(adm(&p, &listed, false), Err(Refusal::ModeOff));
    let follow = person("3", "mallory", false, "th");
    assert_eq!(adm(&p, &follow, true), Err(Refusal::ModeOff));
}

#[test]
fn a_followup_by_a_listed_person_is_explicit_and_by_an_unlisted_person_inferred() {
    let p = with_mode(AcceptMode::Auto);
    let a = person("1", "alice", false, "th");
    assert_eq!(adm(&p, &a, true), Ok((Strength::Explicit, Basis::Followup)));
    let m = person("2", "mallory", false, "th");
    assert_eq!(adm(&p, &m, true), Ok((Strength::Inferred, Basis::Inferred)));
}

#[test]
fn ci_has_no_sender_and_uses_only_the_last_cell() {
    let mut p = with_mode(AcceptMode::Auto);
    let ci = trigger("1", None, false, "th", "all");
    assert_eq!(adm(&p, &ci, false), Err(Refusal::Ignored));
    p.sources
        .get_mut("gh")
        .unwrap()
        .scopes
        .get_mut("all")
        .unwrap()
        .admission
        .unlisted_unaddressed = Cell::Inferred;
    assert_eq!(
        adm(&p, &ci, false),
        Ok((
            Strength::Inferred,
            Basis::Table(CellRef::UnlistedUnaddressed)
        ))
    );
    let quiet = trigger("2", None, false, "th", "issues");
    assert_eq!(adm(&p, &quiet, false), Err(Refusal::NotReceivedAll));
}

#[test]
fn unknown_source_unknown_scope_and_a_silenced_source_fail_closed() {
    let mut p = policy();
    let mut t = person("1", "alice", true, "th");
    t.scope = "nowhere".into();
    assert_eq!(adm(&p, &t, false), Err(Refusal::UnknownScope));
    t.scope = "issues".into();
    t.source = "feishu".into();
    assert_eq!(adm(&p, &t, false), Err(Refusal::UnknownSource));
    p.sources.get_mut("gh").unwrap().listening = false;
    assert_eq!(
        adm(&p, &person("2", "alice", true, "th"), false),
        Err(Refusal::SourceOff)
    );
}

#[test]
fn exhaustive_table_never_grants_a_bot_explicit_or_an_inferred_case_in_off_mode() {
    let cells = [Cell::Explicit, Cell::Inferred, Cell::Ignore];
    let modes = [AcceptMode::Off, AcceptMode::Observe, AcceptMode::Auto];
    let actors: [Option<(&str, bool)>; 5] = [
        None,
        Some(("alice", false)),
        Some(("mallory", false)),
        Some(("dependabot[bot]", true)),
        Some(("evil[bot]", true)),
    ];
    let mut checked = 0u32;
    for mode in modes {
        for a in cells {
            for b in cells {
                for c in cells {
                    for receive_all in [false, true] {
                        for addressed in [false, true] {
                            for ticket in [false, true] {
                                for actor in actors {
                                    let mut p = with_mode(mode);
                                    let sc = p
                                        .sources
                                        .get_mut("gh")
                                        .unwrap()
                                        .scopes
                                        .get_mut("issues")
                                        .unwrap();
                                    sc.receive_all = receive_all;
                                    sc.admission = Admission {
                                        listed_unaddressed: a,
                                        unlisted_addressed: b,
                                        unlisted_unaddressed: c,
                                    };
                                    let t = trigger("x", actor, addressed, "th", "issues");
                                    if let Ok(g) = gate(&p, &t, ticket) {
                                        if t.is_bot() {
                                            assert_eq!(g.strength, Strength::Inferred);
                                        }
                                        if mode == AcceptMode::Off {
                                            assert_eq!(g.strength, Strength::Explicit);
                                        }
                                    }
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3 * 27 * 2 * 2 * 2 * 5);
}

#[test]
fn revalidation_follows_the_mandate_basis() {
    let mut p = with_mode(AcceptMode::Auto);
    let mk = |basis, strength, rules: &[&str], sender: &str| Mandate {
        policy_version: 1,
        scope: "work".into(),
        strength,
        rules: rules.iter().map(|s| s.to_string()).collect(),
        sender: Some(sender.into()),
        source: "gh".into(),
        receiving_scope: "issues".into(),
        thread: ThreadId::new("th"),
        basis,
    };
    let listed = mk(Basis::Listed, Strength::Explicit, &[], "alice");
    let followup = mk(Basis::Followup, Strength::Explicit, &[], "alice");
    let botm = mk(Basis::Bot, Strength::Inferred, &["deps"], "dependabot[bot]");
    let inferred = mk(Basis::Inferred, Strength::Inferred, &["deps"], "mallory");
    for m in [&listed, &followup, &botm, &inferred] {
        assert_eq!(revalidate(&p, m), Ok(()));
    }
    p.sources.get_mut("gh").unwrap().allowed.clear();
    assert_eq!(revalidate(&p, &listed), Err(Revoke::SenderRemoved));
    assert_eq!(revalidate(&p, &followup), Ok(()));
    p.sources.get_mut("gh").unwrap().bots.clear();
    assert_eq!(revalidate(&p, &botm), Err(Revoke::BotRemoved));
    p.mode = AcceptMode::Observe;
    assert_eq!(revalidate(&p, &inferred), Err(Revoke::ProactiveOff));
    p.mode = AcceptMode::Auto;
    p.rules.get_mut("deps").unwrap().enabled = false;
    assert_eq!(revalidate(&p, &inferred), Err(Revoke::RuleDisabled));
    p.sources.get_mut("gh").unwrap().listening = false;
    assert_eq!(revalidate(&p, &followup), Err(Revoke::SourceGone));
}

#[test]
fn a_table_mandate_dies_when_its_cell_weakens_or_its_listed_sender_leaves() {
    let mut p = with_mode(AcceptMode::Auto);
    let mk = |at, strength| Mandate {
        policy_version: 1,
        scope: "work".into(),
        strength,
        rules: vec!["deps".into()],
        sender: Some("alice".into()),
        source: "gh".into(),
        receiving_scope: "issues".into(),
        thread: ThreadId::new("th"),
        basis: Basis::Table(at),
    };
    let listed_cell = mk(CellRef::ListedUnaddressed, Strength::Inferred);
    assert_eq!(revalidate(&p, &listed_cell), Ok(()));
    let ex = mk(CellRef::UnlistedAddressed, Strength::Explicit);
    assert_eq!(revalidate(&p, &ex), Err(Revoke::CellWeakened));
    p.sources
        .get_mut("gh")
        .unwrap()
        .scopes
        .get_mut("issues")
        .unwrap()
        .admission
        .unlisted_addressed = Cell::Explicit;
    assert_eq!(revalidate(&p, &ex), Ok(()));
    p.sources
        .get_mut("gh")
        .unwrap()
        .scopes
        .get_mut("issues")
        .unwrap()
        .admission
        .unlisted_addressed = Cell::Inferred;
    assert_eq!(revalidate(&p, &ex), Err(Revoke::CellWeakened));
    p.sources.get_mut("gh").unwrap().allowed.clear();
    assert_eq!(revalidate(&p, &listed_cell), Err(Revoke::SenderRemoved));
}

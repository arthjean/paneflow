#![allow(clippy::expect_used)]

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};
use paneflow_agent_config::screen_rules::{
    evaluate, parse_base_rules, Evaluation, RuleOrigin, ScreenInput, ScreenRule,
};

fn twenty_rules_and_a_200x60_screen() -> (Vec<ScreenRule>, String) {
    let mut text = String::from("engine = 2\n");
    for index in 0..20 {
        text.push_str(&format!(
            "[[rules]]\nid = \"rule-{index}\"\nstate = \"working\"\npriority = {index}\nregion = \"{region}\"\nany = ['(?i)marker-{index} \\(', '(?m)^\\s*❯ {index}']\nnot = ['(?i)to view']\n",
            region = if index % 2 == 0 { "all" } else { "last:15" }
        ));
    }
    let rules = parse_base_rules(&text, RuleOrigin::Builtin).expect("the bench rules must parse");
    let line = "x".repeat(199);
    let screen = (0..60)
        .map(|row| format!("{row:>2}{}", &line[2..]))
        .collect::<Vec<_>>()
        .join("\n");
    (rules, screen)
}

#[library_benchmark]
#[bench::twenty_rules_200x60(setup = twenty_rules_and_a_200x60_screen)]
fn evaluate_rules((rules, screen): (Vec<ScreenRule>, String)) -> Evaluation {
    black_box(evaluate(
        &rules,
        &ScreenInput {
            screen: black_box(&screen),
            ..ScreenInput::default()
        },
    ))
}

library_benchmark_group!(name = screen_rules; benchmarks = evaluate_rules);

main!(library_benchmark_groups = screen_rules);

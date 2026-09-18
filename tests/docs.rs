//! The documentation, checked against the implementation.
//!
//! Prose goes stale quietly. Every complete `canopy.yaml` shown in the docs is parsed here, so
//! an example that stops being valid fails the build instead of misleading whoever copies it.

use canopyd::parse_str;

/// Every fenced ```yaml block in a markdown file.
fn yaml_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        if line.trim_start_matches('>').trim() != "```yaml" {
            continue;
        }
        let mut body = String::new();
        for (_, line) in lines.by_ref() {
            if line.trim() == "```" {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        blocks.push((index + 1, body));
    }
    blocks
}

/// A block is a whole config if it starts with `version:`; anything else is a fragment
/// illustrating one key, which cannot be validated on its own.
fn complete_configs(markdown: &str) -> Vec<(usize, String)> {
    yaml_blocks(markdown).into_iter().filter(|(_, body)| body.trim_start().starts_with("version:")).collect()
}

#[track_caller]
fn check_examples(file: &str, markdown: &str, expect_at_least: usize) {
    let configs = complete_configs(markdown);
    assert!(
        configs.len() >= expect_at_least,
        "{file}: expected at least {expect_at_least} complete config example(s), found {} — \
         did the examples move, or did the fence language change?",
        configs.len()
    );

    for (line, body) in configs {
        let parsed = parse_str(&body);
        let errors: Vec<String> = parsed.errors().map(|d| format!("{}: {}", d.path, d.message)).collect();
        assert!(errors.is_empty(), "{file}:{line}: example does not parse: {errors:?}");

        // Warnings matter too: an example that trips the linter teaches the wrong habit to
        // whoever copies it, which is most readers.
        let warnings: Vec<String> = parsed.warnings().map(|d| format!("{}: {}", d.path, d.message)).collect();
        assert!(warnings.is_empty(), "{file}:{line}: example produces warnings: {warnings:?}");
    }
}

#[test]
fn readme_config_examples_are_valid() {
    check_examples("README.md", include_str!("../README.md"), 1);
}

#[test]
fn configuration_reference_examples_are_valid() {
    check_examples("docs/configuration.md", include_str!("../docs/configuration.md"), 2);
}

#[test]
fn the_starter_shown_by_config_init_is_the_one_documented() {
    // `config init` prints this; the configuration reference describes its defaults. If the
    // two drift, the first thing a new user does is read a description of something else.
    let parsed = parse_str(canopyd::config::STARTER);
    assert!(parsed.is_valid(), "{:?}", parsed.errors().collect::<Vec<_>>());
    assert_eq!(parsed.warning_count(), 0);
}

#[test]
fn every_documented_error_code_exists() {
    // The JSON reference tabulates error codes; a code that is documented but not real sends
    // a consumer hunting for a branch that can never be taken.
    let doc = section(include_str!("../docs/json-api.md"), "## Errors");
    let known: Vec<&str> = canopyd::ErrorCode::ALL.iter().map(|code| code.as_str()).collect();

    let mut documented = Vec::new();
    for line in doc.lines().filter(|line| line.starts_with("| `")) {
        if let Some(code) = line.trim_start_matches("| `").split('`').next()
            && (known.contains(&code) || code.contains('_'))
        {
            documented.push(code.to_owned());
        }
    }

    assert!(documented.len() >= known.len(), "only {} of {} codes documented", documented.len(), known.len());
    for code in &documented {
        assert!(known.contains(&code.as_str()), "docs/json-api.md documents `{code}`, which is not a real ErrorCode");
    }
    for code in &known {
        assert!(documented.iter().any(|d| d == code), "error code `{code}` is not documented in docs/json-api.md");
    }
}

/// The text under `heading`, up to the next heading at the same level or above.
///
/// The JSON reference holds more than one table of backticked names. Each check reads the one
/// it is about, so a row in the events table is never mistaken for an error code.
fn section<'a>(markdown: &'a str, heading: &str) -> &'a str {
    let level = heading.chars().take_while(|c| *c == '#').count();
    let start = markdown.find(heading).unwrap_or_else(|| panic!("no {heading:?} section"));
    let body = &markdown[start + heading.len()..];
    let end = body
        .match_indices("\n#")
        .find(|(at, _)| body[at + 1..].chars().take_while(|c| *c == '#').count() <= level)
        .map_or(body.len(), |(at, _)| at);
    &body[..end]
}

#[test]
fn every_run_event_is_documented_and_every_documented_event_is_real() {
    // An embedder switches on `event`. A tag missing from the table is one it never handles, and
    // a tag in the table that nothing emits is a branch it writes for nothing.
    use canopyd::{Event, Exit};
    let name = || "web".to_owned();
    let events = [
        Event::Started { name: name(), pid: 1 },
        Event::Healthy { name: name() },
        Event::Unhealthy { name: name(), detail: String::new() },
        Event::Exited { name: name(), status: Exit::Unknown },
        Event::Restarting { name: name(), attempt: 1, delay_ms: 1 },
        Event::GaveUp { name: name(), restarts: 1 },
        Event::Stopped { name: name() },
        Event::Rejected { request: String::new(), detail: String::new() },
    ];
    let real: Vec<String> =
        events.iter().map(|event| serde_json::to_value(event).unwrap()["event"].as_str().unwrap().to_owned()).collect();

    let doc = section(include_str!("../docs/json-api.md"), "### `run`: events while it runs");
    let documented: Vec<&str> = doc
        .lines()
        .filter(|line| line.starts_with("| `"))
        .filter_map(|line| line.trim_start_matches("| `").split('`').next())
        .filter(|tag| *tag != "event")
        .collect();

    for tag in &real {
        assert!(documented.contains(&tag.as_str()), "event `{tag}` is not documented in docs/json-api.md");
    }
    for tag in &documented {
        assert!(real.iter().any(|r| r == tag), "docs/json-api.md documents event `{tag}`, which nothing emits");
    }
}

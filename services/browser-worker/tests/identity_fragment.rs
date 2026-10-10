//! The shipped permission stanza grants exactly what the browser worker uses (XR-021 CONTRACTS.md S03).

const FRAGMENT: &str = include_str!("../../../deploy/nats/identity-browser-worker.conf");

/// The publish allowlist of the `EXTRACTOR_BROWSER_WORKER` stanza, in the order CONTRACTS.md S03 states it.
const PUBLISH_ALLOW: [&str; 8] = [
    "evt.content.render.completed.v1",
    "evt.content.render.failed.v1",
    "$JS.API.CONSUMER.INFO.ratatoskr_commands.ratatoskr_browser_worker",
    "$JS.API.CONSUMER.MSG.NEXT.ratatoskr_commands.ratatoskr_browser_worker",
    "$JS.ACK.ratatoskr_commands.ratatoskr_browser_worker.>",
    "$JS.API.STREAM.INFO.KV_browser_worker_completions",
    "$JS.API.DIRECT.GET.KV_browser_worker_completions.>",
    "$KV.browser_worker_completions.>",
];

/// Comments and whitespace removed and the nkey token dropped: the form the workspace check
/// compares against Platform's `ratatoskr.conf`.
fn normalized(text: &str) -> String {
    let mut tokens = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::split_whitespace);
    let mut output = String::new();
    while let Some(token) = tokens.next() {
        output.push_str(token);
        if token == "nkey:" {
            let _ = tokens.next();
        }
    }
    output
}

#[test]
fn fragment_has_no_broad_grants() {
    let stanza = normalized(FRAGMENT);
    for broad in [
        "\"evt.>\"",
        "\"cmd.>\"",
        "\"$JS.API.>\"",
        "\"$JS.ACK.>\"",
        "\"$KV.>\"",
        "deny",
    ] {
        assert!(
            !stanza.contains(broad),
            "the browser worker stanza must not contain {broad}"
        );
    }
    let allow = PUBLISH_ALLOW.iter().fold(String::new(), |list, subject| {
        format!("{list}\"{subject}\",")
    });
    assert_eq!(
        stanza,
        format!(
            "{{nkey:permissions:{{publish:{{allow:[{allow}]}}subscribe:{{allow:[\"_INBOX.>\"]}}}}}}"
        )
    );
}

#[test]
fn the_nkey_is_a_unique_public_placeholder() {
    const PREFIX: &str = "UREPLACE_ME_WITH_THE_PUBLIC_NKEY_OF_RATATOSKR_EXTRACTOR_BROWSER_WORKER_";
    let tokens: Vec<&str> = FRAGMENT
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::split_whitespace)
        .filter(|token| token.starts_with("UREPLACE_ME_"))
        .collect();
    assert_eq!(tokens.len(), 1);
    let suffix = tokens.first().and_then(|token| token.strip_prefix(PREFIX));
    assert!(
        suffix.is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c == 'X')),
        "the placeholder must be {PREFIX} followed by X characters"
    );
}
